use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_associated_token_account::instruction::create_associated_token_account_idempotent;
use spl_token::instruction::transfer_checked;
use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, RwLock},
};
use tokio::time::{Duration, sleep};
use tower_http::cors::CorsLayer;

#[derive(Clone, Serialize)]
struct Listing {
    parcel_id: String,
    mint: String,
    seller: String,
    price_lamports: u64,
    decimals: u8,
}

#[derive(Clone)]
struct AppState {
    rpc: Arc<RpcClient>,
    seller: Arc<Keypair>,
    listings: Arc<RwLock<HashMap<String, Listing>>>,
}

#[derive(Deserialize)]
struct PurchaseRequest {
    buyer: String,
}

#[derive(Serialize)]
struct PurchaseResponse {
    parcel_id: String,
    blockhash: String,
    partially_signed_transaction: String,
    seller: String,
    price_lamports: u64,
}

#[derive(Deserialize)]
struct ConfirmRequest {
    buyer: String,
    signature: String,
}

#[derive(Serialize)]
struct ConfirmResponse {
    verified: bool,
    message: String,
}

#[derive(Debug)]
struct ApiError(String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response { (StatusCode::BAD_REQUEST, self.0).into_response() }
}

fn parse_pubkey(value: &str, field: &str) -> Result<Pubkey, ApiError> {
    Pubkey::from_str(value).map_err(|_| ApiError(format!("invalid {field} public key")))
}

fn seller_keypair() -> Result<Keypair> {
    let encoded = if let Ok(path) = env::var("VELOREN_MARKETPLACE_SELLER_KEYPAIR_FILE") {
        std::fs::read_to_string(path).context("could not read seller keypair file")?
    } else {
        env::var("VELOREN_MARKETPLACE_SELLER_KEYPAIR")
            .context("VELOREN_MARKETPLACE_SELLER_KEYPAIR or _FILE is required")?
    };
    let bytes: Vec<u8> =
        serde_json::from_str(&encoded).context("seller keypair must be a JSON byte array")?;
    Keypair::try_from(bytes.as_slice()).context("seller keypair bytes are invalid")
}

fn listing_from_env() -> Result<Listing> {
    Ok(Listing {
        parcel_id: env::var("VELOREN_MARKETPLACE_PARCEL_ID")
            .unwrap_or_else(|_| "DEV-LAND-0002".to_string()),
        mint: env::var("VELOREN_MARKETPLACE_LAND_MINT")
            .context("VELOREN_MARKETPLACE_LAND_MINT is required")?,
        seller: env::var("VELOREN_MARKETPLACE_SELLER")
            .context("VELOREN_MARKETPLACE_SELLER is required")?,
        price_lamports: env::var("VELOREN_MARKETPLACE_PRICE_LAMPORTS")
            .unwrap_or_else(|_| "1000000".to_string())
            .parse()
            .context("VELOREN_MARKETPLACE_PRICE_LAMPORTS must be an integer")?,
        decimals: env::var("VELOREN_MARKETPLACE_MINT_DECIMALS")
            .unwrap_or_else(|_| "0".to_string())
            .parse()
            .context("VELOREN_MARKETPLACE_MINT_DECIMALS must be an integer")?,
    })
}

async fn list_listings(State(state): State<AppState>) -> Json<Vec<Listing>> {
    Json(
        state
            .listings
            .read()
            .expect("listing lock poisoned")
            .values()
            .cloned()
            .collect(),
    )
}

async fn purchase(
    Path(parcel_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<PurchaseRequest>,
) -> Result<Json<PurchaseResponse>, ApiError> {
    let listing = state
        .listings
        .read()
        .map_err(|_| ApiError("listing state unavailable".to_string()))?
        .get(&parcel_id)
        .cloned()
        .ok_or_else(|| ApiError("property listing was not found".to_string()))?;
    let buyer = parse_pubkey(&request.buyer, "buyer")?;
    let seller = parse_pubkey(&listing.seller, "seller")?;
    if seller != state.seller.pubkey() {
        return Err(ApiError(
            "configured seller does not own this listing".to_string(),
        ));
    }
    if buyer == seller {
        return Err(ApiError("buyer already owns this listing".to_string()));
    }

    let mint = parse_pubkey(&listing.mint, "land mint")?;
    let seller_ata = spl_associated_token_account::get_associated_token_address(&seller, &mint);
    let buyer_ata = spl_associated_token_account::get_associated_token_address(&buyer, &mint);
    let seller_balance = state
        .rpc
        .get_token_account_balance(&seller_ata)
        .map_err(|error| ApiError(format!("could not verify seller NFT account: {error}")))?;
    if seller_balance.amount != "1" {
        return Err(ApiError(
            "seller does not hold exactly one token for this listing".to_string(),
        ));
    }
    if let Ok(buyer_balance) = state.rpc.get_token_account_balance(&buyer_ata) {
        if buyer_balance.amount != "0" {
            return Err(ApiError("buyer already owns this land mint".to_string()));
        }
    }
    let blockhash = state
        .rpc
        .get_latest_blockhash()
        .map_err(|error| ApiError(format!("could not fetch Solana blockhash: {error}")))?;
    let instructions: Vec<Instruction> = vec![
        create_associated_token_account_idempotent(&seller, &buyer, &mint, &spl_token::id()),
        system_instruction::transfer(&buyer, &seller, listing.price_lamports),
        transfer_checked(
            &spl_token::id(),
            &seller_ata,
            &mint,
            &buyer_ata,
            &seller,
            &[],
            1,
            listing.decimals,
        )
        .map_err(|error| ApiError(format!("could not prepare NFT transfer: {error}")))?,
    ];
    let message = Message::new(&instructions, Some(&buyer));
    let mut transaction = Transaction::new_unsigned(message);
    transaction
        .try_partial_sign(&[state.seller.as_ref()], blockhash)
        .map_err(|error| ApiError(format!("could not sign marketplace transaction: {error}")))?;
    let serialized = bincode::serialize(&transaction)
        .map_err(|error| ApiError(format!("could not serialize transaction: {error}")))?;
    Ok(Json(PurchaseResponse {
        parcel_id,
        blockhash: blockhash.to_string(),
        partially_signed_transaction: STANDARD.encode(serialized),
        seller: listing.seller,
        price_lamports: listing.price_lamports,
    }))
}

async fn confirm(
    Path(parcel_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<ConfirmRequest>,
) -> Result<Json<ConfirmResponse>, ApiError> {
    let listing = state
        .listings
        .read()
        .map_err(|_| ApiError("listing state unavailable".to_string()))?
        .get(&parcel_id)
        .cloned()
        .ok_or_else(|| ApiError("property listing was not found".to_string()))?;
    let buyer = parse_pubkey(&request.buyer, "buyer")?;
    let signature = Signature::from_str(&request.signature)
        .map_err(|_| ApiError("invalid transaction signature".to_string()))?;
    let mut status = None;
    for _ in 0..30 {
        match state
            .rpc
            .get_signature_status_with_commitment(&signature, CommitmentConfig::confirmed())
        {
            Ok(Some(result)) => {
                status = Some(result);
                break;
            },
            Ok(None) => sleep(Duration::from_millis(500)).await,
            Err(error) => {
                return Err(ApiError(format!(
                    "could not query transaction status: {error}"
                )));
            },
        }
    }
    match status {
        Some(Ok(())) => {},
        Some(Err(error)) => {
            return Err(ApiError(format!("transaction failed on Solana: {error}")));
        },
        None => {
            return Err(ApiError(
                "transaction was not confirmed by devnet before timeout".to_string(),
            ));
        },
    }
    let mint = parse_pubkey(&listing.mint, "land mint")?;
    let buyer_ata = spl_associated_token_account::get_associated_token_address(&buyer, &mint);
    let owns_mint = state
        .rpc
        .get_token_account_balance(&buyer_ata)
        .ok()
        .is_some_and(|balance| balance.amount == "1");
    Ok(Json(ConfirmResponse {
        verified: owns_mint,
        message: if owns_mint {
            "Solana transaction confirmed and land ownership verified.".to_string()
        } else {
            "Transaction confirmed, but the buyer does not own the configured land NFT.".to_string()
        },
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let bind: SocketAddr = env::var("VELOREN_MARKETPLACE_BIND")
        .unwrap_or_else(|_| "127.0.0.1:19254".to_string())
        .parse()
        .context("VELOREN_MARKETPLACE_BIND must be host:port")?;
    let rpc_url = env::var("VELOREN_SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    let seller = Arc::new(seller_keypair()?);
    let listing = listing_from_env()?;
    if listing.seller != seller.pubkey().to_string() {
        return Err(anyhow!(
            "VELOREN_MARKETPLACE_SELLER must match the configured keypair"
        ));
    }
    let rpc = Arc::new(RpcClient::new_with_commitment(
        rpc_url,
        CommitmentConfig::confirmed(),
    ));
    let listings = Arc::new(RwLock::new(HashMap::from([(
        listing.parcel_id.clone(),
        listing,
    )])));
    let state = AppState {
        rpc,
        seller,
        listings,
    };
    let app = Router::new()
        .route("/marketplace/listings", get(list_listings))
        .route("/marketplace/{parcel_id}/purchase", post(purchase))
        .route("/marketplace/{parcel_id}/confirm", post(confirm))
        .layer(CorsLayer::permissive())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("Veloren marketplace service listening on {bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
