use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorldPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParcelBounds {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PropertyParcel {
    pub id: String,
    pub land_nft_id: String,
    pub world_id: String,
    pub continent: String,
    pub region: String,
    pub land_type: String,
    pub rarity: String,
    pub terrain_type: String,
    pub water_access: bool,
    pub road_access: bool,
    pub price_vgld_base_units: u64,
    pub protected: bool,
    pub bounds: ParcelBounds,
    pub allowed_buildings: Vec<String>,
    pub max_buildings: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BuildingFootprint {
    pub width: i32,
    pub height: i32,
    pub clearance: i32,
    pub capacity_cost: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedZone {
    pub id: String,
    pub world_id: String,
    pub bounds: ParcelBounds,
    pub reason: String,
}

impl BuildingFootprint {
    pub const fn new(width: i32, height: i32, clearance: i32, capacity_cost: usize) -> Self {
        Self {
            width,
            height,
            clearance,
            capacity_cost,
        }
    }

    pub const fn small() -> Self { Self::new(12, 12, 4, 1) }

    pub const fn medium() -> Self { Self::new(18, 18, 5, 1) }

    pub const fn large() -> Self { Self::new(24, 24, 6, 2) }

    pub const fn very_large() -> Self { Self::new(34, 34, 8, 3) }

    pub fn for_building_type(building_type: &str) -> Self {
        match building_type {
            "castle" | "citadel" | "guild_hall" | "fortress" | "town_hall" => Self::very_large(),
            "inn" | "barn" | "stable" | "farmhouse" | "blacksmith" | "shop" => Self::large(),
            "house" | "workshop" | "storage" | "market" => Self::medium(),
            _ => Self::small(),
        }
    }

    pub fn overlaps_blocked_area(
        &self,
        other: &BuildingFootprint,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
    ) -> bool {
        let min_x1 = x1 - self.width / 2 - self.clearance;
        let max_x1 = x1 + self.width / 2 + self.clearance;
        let min_y1 = y1 - self.height / 2 - self.clearance;
        let max_y1 = y1 + self.height / 2 + self.clearance;

        let min_x2 = x2 - other.width / 2 - other.clearance;
        let max_x2 = x2 + other.width / 2 + other.clearance;
        let min_y2 = y2 - other.height / 2 - other.clearance;
        let max_y2 = y2 + other.height / 2 + other.clearance;

        min_x1 <= max_x2 && max_x1 >= min_x2 && min_y1 <= max_y2 && max_y1 >= min_y2
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildingAsset {
    pub id: String,
    pub building_type: String,
    pub game_asset_id: String,
    pub collection: String,
    pub footprint: BuildingFootprint,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlacedBuilding {
    pub x: i32,
    pub y: i32,
    pub building_type: String,
    pub footprint: BuildingFootprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NftMetadata {
    pub token_id: String,
    pub collection: String,
    pub asset_type: String,
    pub land_type: Option<String>,
    pub building_type: Option<String>,
    pub world_id: Option<String>,
    pub metadata_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyError {
    WalletNotLinked,
    InvalidChallenge,
    SignatureInvalid,
    LandOwnershipMissing,
    LandMetadataMismatch,
    BuildingOwnershipMissing,
    ParcelNotFound,
    ProtectedZone,
    BuildingTypeNotAllowed,
    BuildingPlacementOutOfBounds,
    BuildingPlacementConflict,
    ParcelFull,
    PurchaseReserved,
    UnknownAsset,
    PersistenceFailed,
}

impl std::fmt::Display for PropertyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WalletNotLinked => write!(f, "wallet is not linked to this player"),
            Self::InvalidChallenge => write!(f, "wallet challenge is invalid or expired"),
            Self::SignatureInvalid => write!(f, "wallet signature is invalid"),
            Self::LandOwnershipMissing => write!(f, "land ownership was not verified"),
            Self::LandMetadataMismatch => {
                write!(f, "land NFT metadata does not match this parcel")
            },
            Self::BuildingOwnershipMissing => write!(f, "building ownership was not verified"),
            Self::ParcelNotFound => write!(f, "parcel was not found"),
            Self::ProtectedZone => write!(f, "this area is protected from player ownership"),
            Self::BuildingTypeNotAllowed => write!(f, "this building type is not allowed here"),
            Self::BuildingPlacementOutOfBounds => {
                write!(f, "building placement is outside parcel bounds")
            },
            Self::BuildingPlacementConflict => {
                write!(
                    f,
                    "building footprint overlaps another building or required clearance"
                )
            },
            Self::ParcelFull => write!(f, "this parcel is full"),
            Self::PurchaseReserved => write!(f, "this parcel is reserved for another purchase"),
            Self::UnknownAsset => write!(f, "asset metadata is missing or invalid"),
            Self::PersistenceFailed => write!(f, "failed to persist property placement"),
        }
    }
}

impl std::error::Error for PropertyError {}

pub type PropertyResult<T> = Result<T, PropertyError>;

pub fn wallet_link_message(wallet: &str, challenge: &str) -> String {
    format!("Veloren wallet link\nWallet: {wallet}\nChallenge: {challenge}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockchainConfig {
    pub network: String,
    pub rpc_url: String,
    pub land_collection: String,
    pub building_collection: String,
}

impl Default for BlockchainConfig {
    fn default() -> Self {
        Self {
            network: "devnet".to_string(),
            rpc_url: "https://api.devnet.solana.com".to_string(),
            land_collection: "land".to_string(),
            building_collection: "building".to_string(),
        }
    }
}

impl BlockchainConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(value) = std::env::var("VELOREN_SOLANA_NETWORK") {
            config.network = value;
        }
        if let Ok(value) = std::env::var("VELOREN_SOLANA_RPC_URL") {
            config.rpc_url = value;
        }
        if let Ok(value) = std::env::var("VELOREN_LAND_COLLECTION") {
            config.land_collection = value;
        }
        if let Ok(value) = std::env::var("VELOREN_BUILDING_COLLECTION") {
            config.building_collection = value;
        }
        config
    }
}

pub trait BlockchainProvider: Send + Sync {
    fn sandbox_assign_land(&mut self, _wallet: &str, _land_nft_id: &str) -> bool { false }

    fn verify_land_ownership(
        &self,
        wallet: &str,
        land_nft_id: &str,
        expected_collection: &str,
    ) -> bool;

    fn verify_building_ownership(
        &self,
        wallet: &str,
        building_nft_id: &str,
        expected_collection: &str,
    ) -> bool;

    fn get_land_metadata(&self, land_nft_id: &str) -> Option<NftMetadata>;

    fn get_building_metadata(&self, building_nft_id: &str) -> Option<NftMetadata>;

    fn get_owner(&self, nft_id: &str) -> Option<String>;
}

#[derive(Clone, Debug, Default)]
pub struct SolanaBlockchainProvider {
    config: BlockchainConfig,
    wallet_nfts: HashMap<String, HashSet<String>>,
    land_metadata: HashMap<String, NftMetadata>,
    building_metadata: HashMap<String, NftMetadata>,
}

impl SolanaBlockchainProvider {
    pub fn new() -> Self { Self::with_config(BlockchainConfig::default()) }

    pub fn with_config(config: BlockchainConfig) -> Self {
        Self {
            config,
            wallet_nfts: HashMap::new(),
            land_metadata: HashMap::new(),
            building_metadata: HashMap::new(),
        }
    }

    pub fn config(&self) -> &BlockchainConfig { &self.config }

    pub fn set_land_metadata(
        &mut self,
        nft_id: impl Into<String>,
        wallet: impl Into<String>,
        metadata: NftMetadata,
    ) {
        let nft_id = nft_id.into();
        let wallet = wallet.into();
        self.wallet_nfts
            .entry(wallet)
            .or_default()
            .insert(nft_id.clone());
        self.land_metadata.insert(nft_id, metadata);
    }

    pub fn set_building_metadata(
        &mut self,
        nft_id: impl Into<String>,
        wallet: impl Into<String>,
        metadata: NftMetadata,
    ) {
        let nft_id = nft_id.into();
        let wallet = wallet.into();
        self.wallet_nfts
            .entry(wallet)
            .or_default()
            .insert(nft_id.clone());
        self.building_metadata.insert(nft_id, metadata);
    }

    fn wallet_owns_nft(&self, wallet: &str, nft_id: &str) -> bool {
        self.wallet_nfts
            .get(wallet)
            .is_some_and(|owned| owned.contains(nft_id))
            || self
                .query_wallet_nft_ownership(wallet, nft_id)
                .unwrap_or(false)
    }

    fn query_wallet_nft_ownership(&self, wallet: &str, nft_id: &str) -> Result<bool, String> {
        if self.config.rpc_url.trim().is_empty() {
            return Err("Solana RPC URL is not configured".to_string());
        }

        let response = Client::new()
            .post(&self.config.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getTokenAccountsByOwner",
                "params": [
                    wallet,
                    { "programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" },
                    { "encoding": "jsonParsed" }
                ]
            }))
            .send()
            .map_err(|err| err.to_string())?;

        if !response.status().is_success() {
            return Err(format!("RPC request failed: {}", response.status()));
        }

        let value: serde_json::Value = response.json().map_err(|err| err.to_string())?;

        let owned = value
            .get("result")
            .and_then(|result| result.get("value"))
            .and_then(|value| value.as_array())
            .map(|accounts| {
                accounts.iter().any(|account| {
                            let info = account
                        .get("account")
                        .and_then(|account| account.get("data"))
                        .and_then(|data| data.get("parsed"))
                                .and_then(|parsed| parsed.get("info"));
                            info.and_then(|info| info.get("mint"))
                                .and_then(|mint| mint.as_str())
                                .is_some_and(|mint| mint == nft_id)
                                && info.and_then(|info| info.pointer("/tokenAmount/amount"))
                                    .and_then(Value::as_str)
                                    == Some("1")
                                && info.and_then(|info| info.pointer("/tokenAmount/decimals"))
                                    .and_then(Value::as_u64)
                                    == Some(0)
                })
            })
            .unwrap_or(false);

        Ok(owned)
    }

    fn query_asset_metadata(&self, nft_id: &str, asset_type: &str) -> Option<NftMetadata> {
        if self.config.rpc_url.trim().is_empty() {
            return None;
        }

        let response = Client::new()
            .post(&self.config.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getAsset",
                "params": { "id": nft_id }
            }))
            .send()
            .ok()?;

        if !response.status().is_success() {
            return None;
        }

        let value: serde_json::Value = response.json().ok()?;
        let result = value.get("result")?;
        let metadata = result.get("content")?.get("metadata")?;
        let attributes = metadata.get("attributes").and_then(Value::as_array);
        let trait_value = |trait_type: &str| {
            attributes.and_then(|attributes| {
                attributes.iter().find_map(|attribute| {
                    (attribute.get("trait_type")?.as_str()? == trait_type)
                        .then(|| attribute.get("value")?.as_str().map(str::to_owned))
                        .flatten()
                })
            })
        };
        let collection = result
            .get("grouping")
            .and_then(Value::as_array)
            .and_then(|groups| {
                groups.iter().find_map(|group| {
                    (group.get("group_key")?.as_str()? == "collection")
                        .then(|| group.get("group_value")?.as_str().map(str::to_owned))
                        .flatten()
                })
            })?;
        let metadata_uri = result
            .get("content")
            .and_then(|content| content.get("json_uri"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        Some(NftMetadata {
            token_id: nft_id.to_owned(),
            collection,
            asset_type: trait_value("asset_type").unwrap_or_else(|| asset_type.to_owned()),
            land_type: trait_value("land_type"),
            building_type: trait_value("building_type"),
            world_id: trait_value("world_id"),
            metadata_uri,
        })
    }
}

impl BlockchainProvider for SolanaBlockchainProvider {
    fn verify_land_ownership(
        &self,
        wallet: &str,
        land_nft_id: &str,
        _expected_collection: &str,
    ) -> bool {
        self.wallet_owns_nft(wallet, land_nft_id)
    }

    fn verify_building_ownership(
        &self,
        wallet: &str,
        building_nft_id: &str,
        expected_collection: &str,
    ) -> bool {
        let metadata = self
            .building_metadata
            .get(building_nft_id)
            .cloned()
            .or_else(|| self.query_asset_metadata(building_nft_id, "building"));

        let expected_collection = if expected_collection.is_empty() {
            &self.config.building_collection
        } else {
            expected_collection
        };

        metadata.is_some_and(|metadata| {
            metadata.asset_type == "building"
                && metadata.collection == expected_collection
                && self.wallet_owns_nft(wallet, building_nft_id)
        })
    }

    fn get_land_metadata(&self, land_nft_id: &str) -> Option<NftMetadata> {
        self.land_metadata
            .get(land_nft_id)
            .cloned()
            .or_else(|| self.query_asset_metadata(land_nft_id, "land"))
    }

    fn get_building_metadata(&self, building_nft_id: &str) -> Option<NftMetadata> {
        self.building_metadata
            .get(building_nft_id)
            .cloned()
            .or_else(|| self.query_asset_metadata(building_nft_id, "building"))
    }

    fn get_owner(&self, nft_id: &str) -> Option<String> {
        for (wallet, nfts) in &self.wallet_nfts {
            if nfts.contains(nft_id) {
                return Some(wallet.clone());
            }
        }
        None
    }
}

#[derive(Clone)]
pub struct MockBlockchainProvider {
    wallet_nfts: HashMap<String, HashSet<String>>,
    land_metadata: HashMap<String, NftMetadata>,
    building_metadata: HashMap<String, NftMetadata>,
    solana: Option<SolanaBlockchainProvider>,
}

impl Default for MockBlockchainProvider {
    fn default() -> Self { Self::new() }
}

impl MockBlockchainProvider {
    pub fn new() -> Self {
        let solana = std::env::var("VELOREN_PROPERTY_PROVIDER")
            .ok()
            .filter(|provider| provider.eq_ignore_ascii_case("solana"))
            .map(|_| SolanaBlockchainProvider::with_config(BlockchainConfig::from_env()));
        Self {
            wallet_nfts: HashMap::new(),
            land_metadata: HashMap::new(),
            building_metadata: HashMap::new(),
            solana,
        }
    }

    pub fn register_land_nft(
        &mut self,
        nft_id: impl Into<String>,
        wallet: impl Into<String>,
        metadata: NftMetadata,
    ) {
        let nft_id = nft_id.into();
        let wallet = wallet.into();
        self.wallet_nfts
            .entry(wallet)
            .or_default()
            .insert(nft_id.clone());
        self.land_metadata.insert(nft_id, metadata);
    }

    pub fn register_land_metadata(&mut self, nft_id: impl Into<String>, metadata: NftMetadata) {
        self.land_metadata.insert(nft_id.into(), metadata);
    }

    pub fn register_building_nft(
        &mut self,
        nft_id: impl Into<String>,
        wallet: impl Into<String>,
        metadata: NftMetadata,
    ) {
        let nft_id = nft_id.into();
        let wallet = wallet.into();
        self.wallet_nfts
            .entry(wallet)
            .or_default()
            .insert(nft_id.clone());
        self.building_metadata.insert(nft_id, metadata);
    }

    pub fn register_development_nfts(&mut self, wallet: &str) {
        if self.solana.is_some() {
            return;
        }
        self.register_land_nft("land-1", wallet, NftMetadata {
            token_id: "land-1".to_string(),
            collection: "land".to_string(),
            asset_type: "land".to_string(),
            land_type: Some("town".to_string()),
            building_type: None,
            world_id: Some("WORLD-001".to_string()),
            metadata_uri: "ipfs://land-1".to_string(),
        });
        for (token_id, land_type, world_id) in [
            ("land-2", "farm", "WORLD-001"),
            ("land-3", "citadel", "WORLD-001"),
        ] {
            self.register_land_metadata(token_id, NftMetadata {
                token_id: token_id.to_string(),
                collection: "land".to_string(),
                asset_type: "land".to_string(),
                land_type: Some(land_type.to_string()),
                building_type: None,
                world_id: Some(world_id.to_string()),
                metadata_uri: format!("ipfs://{token_id}"),
            });
        }
        self.register_building_nft("building-1", wallet, NftMetadata {
            token_id: "building-1".to_string(),
            collection: "building".to_string(),
            asset_type: "building".to_string(),
            land_type: None,
            building_type: Some("shop".to_string()),
            world_id: None,
            metadata_uri: "ipfs://building-1".to_string(),
        });
    }

    pub fn sandbox_assign_land(&mut self, wallet: &str, land_nft_id: &str) -> bool {
        if self.solana.is_some() || !self.land_metadata.contains_key(land_nft_id) {
            return false;
        }
        self.wallet_nfts
            .entry(wallet.to_string())
            .or_default()
            .insert(land_nft_id.to_string());
        true
    }
}

impl BlockchainProvider for MockBlockchainProvider {
    fn sandbox_assign_land(&mut self, wallet: &str, land_nft_id: &str) -> bool {
        self.sandbox_assign_land(wallet, land_nft_id)
    }

    fn verify_land_ownership(
        &self,
        wallet: &str,
        land_nft_id: &str,
        expected_collection: &str,
    ) -> bool {
        if let Some(provider) = &self.solana {
            return provider.verify_land_ownership(wallet, land_nft_id, expected_collection);
        }
        let Some(metadata) = self.land_metadata.get(land_nft_id) else {
            return false;
        };

        if metadata.asset_type != "land" || metadata.collection != expected_collection {
            return false;
        }

        self.wallet_nfts
            .get(wallet)
            .is_some_and(|owned| owned.contains(land_nft_id))
    }

    fn verify_building_ownership(
        &self,
        wallet: &str,
        building_nft_id: &str,
        expected_collection: &str,
    ) -> bool {
        if let Some(provider) = &self.solana {
            return provider.verify_building_ownership(
                wallet,
                building_nft_id,
                expected_collection,
            );
        }
        let Some(metadata) = self.building_metadata.get(building_nft_id) else {
            return false;
        };

        if metadata.asset_type != "building" || metadata.collection != expected_collection {
            return false;
        }

        self.wallet_nfts
            .get(wallet)
            .is_some_and(|owned| owned.contains(building_nft_id))
    }

    fn get_land_metadata(&self, land_nft_id: &str) -> Option<NftMetadata> {
        if let Some(provider) = &self.solana {
            return provider.get_land_metadata(land_nft_id);
        }
        self.land_metadata.get(land_nft_id).cloned()
    }

    fn get_building_metadata(&self, building_nft_id: &str) -> Option<NftMetadata> {
        if let Some(provider) = &self.solana {
            return provider.get_building_metadata(building_nft_id);
        }
        self.building_metadata.get(building_nft_id).cloned()
    }

    fn get_owner(&self, nft_id: &str) -> Option<String> {
        if let Some(provider) = &self.solana {
            return provider.get_owner(nft_id);
        }
        for (wallet, nfts) in &self.wallet_nfts {
            if nfts.contains(nft_id) {
                return Some(wallet.clone());
            }
        }
        None
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlayerWalletRegistry {
    challenge_ttl: Duration,
    challenges: HashMap<String, (String, Instant)>,
    linked_wallets: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletAccount {
    pub player_id: String,
    pub wallet: String,
}

impl PlayerWalletRegistry {
    pub fn new() -> Self {
        Self {
            challenge_ttl: Duration::from_secs(300),
            challenges: HashMap::new(),
            linked_wallets: HashMap::new(),
        }
    }

    pub fn with_challenge_ttl(mut self, ttl: Duration) -> Self {
        self.challenge_ttl = ttl;
        self
    }

    pub fn issue_challenge(&mut self, player_id: &str) -> String {
        let challenge = format!(
            "challenge:{}:{}",
            player_id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        self.challenges
            .insert(player_id.to_owned(), (challenge.clone(), Instant::now()));
        challenge
    }

    pub fn link_wallet(
        &mut self,
        player_id: &str,
        wallet: &str,
        challenge: &str,
        signature: &str,
    ) -> PropertyResult<()> {
        let Some((expected_challenge, issued_at)) = self.challenges.get(player_id) else {
            return Err(PropertyError::InvalidChallenge);
        };

        if *expected_challenge != challenge {
            return Err(PropertyError::InvalidChallenge);
        }

        if issued_at.elapsed() > self.challenge_ttl {
            self.challenges.remove(player_id);
            return Err(PropertyError::InvalidChallenge);
        }

        let Ok(wallet_bytes) = bs58::decode(wallet).into_vec() else {
            return Err(PropertyError::SignatureInvalid);
        };
        let Ok(wallet_bytes) = <[u8; 32]>::try_from(wallet_bytes.as_slice()) else {
            return Err(PropertyError::SignatureInvalid);
        };
        let Ok(verifying_key) = VerifyingKey::from_bytes(&wallet_bytes) else {
            return Err(PropertyError::SignatureInvalid);
        };
        let Ok(signature_bytes) = bs58::decode(signature).into_vec() else {
            return Err(PropertyError::SignatureInvalid);
        };
        let Ok(signature) = Signature::from_slice(&signature_bytes) else {
            return Err(PropertyError::SignatureInvalid);
        };
        if verifying_key
            .verify(
                wallet_link_message(wallet, challenge).as_bytes(),
                &signature,
            )
            .is_err()
        {
            return Err(PropertyError::SignatureInvalid);
        }

        self.linked_wallets
            .insert(player_id.to_owned(), wallet.to_owned());
        self.challenges.remove(player_id);
        Ok(())
    }

    pub fn linked_wallet(&self, player_id: &str) -> Option<&str> {
        self.linked_wallets.get(player_id).map(String::as_str)
    }

    pub fn account(&self, player_id: &str) -> Option<WalletAccount> {
        self.linked_wallet(player_id).map(|wallet| WalletAccount {
            player_id: player_id.to_owned(),
            wallet: wallet.to_owned(),
        })
    }

    pub fn player_for_wallet(&self, wallet: &str) -> Option<&str> {
        self.linked_wallets
            .iter()
            .find_map(|(player_id, linked_wallet)| {
                (linked_wallet == wallet).then_some(player_id.as_str())
            })
    }
}

pub struct PropertyRuntime<P: BlockchainProvider = MockBlockchainProvider> {
    service: PropertyService<P>,
}

impl Default for PropertyRuntime<MockBlockchainProvider> {
    fn default() -> Self {
        let provider = Arc::new(MockBlockchainProvider::new());
        let mut service = PropertyService::new(provider);
        service.register_default_development_parcels();
        service.register_default_development_buildings();
        Self { service }
    }
}

impl PropertyRuntime<MockBlockchainProvider> {
    pub fn new() -> Self { Self::default() }

    pub fn with_persistence_path(path: impl Into<PathBuf>) -> Self {
        let mut runtime = Self::new();
        let path = path.into();
        runtime.service.set_persistence_path(path.clone());
        runtime
            .service
            .load_or_generate_parcels(path.with_file_name("property_parcels.json"));
        runtime.service.load_placements();
        runtime
    }

    pub fn register_development_nfts(&mut self, wallet: &str) {
        Arc::get_mut(&mut self.service.provider)
            .expect("property provider must be uniquely owned")
            .register_development_nfts(wallet);
    }
}

impl PropertyRuntime<SolanaBlockchainProvider> {
    pub fn new_solana(config: BlockchainConfig) -> Self {
        let provider = Arc::new(SolanaBlockchainProvider::with_config(config));
        let mut service = PropertyService::new(provider);
        service.register_default_development_parcels();
        service.register_default_development_buildings();
        Self { service }
    }

    pub fn new_solana_with_persistence_path(path: impl Into<PathBuf>) -> Self {
        let mut runtime = Self::new_solana(BlockchainConfig::from_env());
        let path = path.into();
        runtime.service.set_persistence_path(path.clone());
        runtime
            .service
            .load_or_generate_parcels(path.with_file_name("property_parcels.json"));
        runtime.service.load_placements();
        runtime
    }
}

impl Default for PropertyRuntime<SolanaBlockchainProvider> {
    fn default() -> Self { Self::new_solana(BlockchainConfig::default()) }
}

pub type ActivePropertyRuntime = PropertyRuntime<SolanaBlockchainProvider>;

impl<P: BlockchainProvider> PropertyRuntime<P> {
    pub fn reload_parcels(&mut self) {
        if let Some(path) = self.service.persistence_path.clone() {
            self.service
                .load_or_generate_parcels(path.with_file_name("property_parcels.json"));
        }
    }

    pub fn parcel_infos(&self) -> Vec<common_net::msg::PropertyParcelInfo> {
        self.service.parcel_infos()
    }

    pub fn parcel_infos_for_player(
        &self,
        player_id: &str,
    ) -> Vec<common_net::msg::PropertyParcelInfo> {
        self.service.parcel_infos_for_player(Some(player_id))
    }

    pub fn with_provider(provider: Arc<P>) -> Self {
        let mut service = PropertyService::new(provider);
        service.register_default_development_parcels();
        service.register_default_development_buildings();
        Self { service }
    }

    pub fn issue_wallet_challenge(&mut self, player_id: &str) -> String {
        self.service.issue_wallet_challenge(player_id)
    }

    pub fn link_wallet(
        &mut self,
        player_id: &str,
        wallet: &str,
        challenge: &str,
        signature: &str,
    ) -> PropertyResult<()> {
        self.service
            .link_wallet(player_id, wallet, challenge, signature)
    }

    pub fn link_verified_wallet(&mut self, player_id: &str, wallet: &str) {
        self.service
            .wallets
            .linked_wallets
            .insert(player_id.to_string(), wallet.to_string());
    }

    pub fn account(&self, player_id: &str) -> Option<WalletAccount> {
        self.service.wallets.account(player_id)
    }

    pub fn sandbox_assign_land(&mut self, player_id: &str, land_nft_id: &str) -> bool {
        let Some(wallet) = self
            .service
            .wallets
            .linked_wallet(player_id)
            .map(str::to_owned)
        else {
            return false;
        };
        self.service.sandbox_assign_land(&wallet, land_nft_id)
    }

    pub fn authorize_placement(
        &mut self,
        player_id: &str,
        parcel_id: &str,
        land_nft_id: &str,
        building_nft_id: &str,
        placement: WorldPosition,
    ) -> PropertyResult<()> {
        self.service.authorize_placement(
            player_id,
            parcel_id,
            land_nft_id,
            building_nft_id,
            placement,
        )
    }

    pub fn building_type_for_placement(&self, building_nft_id: &str) -> Option<String> {
        self.service.building_type_for_placement(building_nft_id)
    }

    pub fn reserve_purchase(&mut self, player_id: &str, parcel_id: &str) -> PropertyResult<u64> {
        self.service.reserve_purchase(player_id, parcel_id)
    }

    pub fn release_purchase(&mut self, parcel_id: &str) {
        self.service.release_purchase(parcel_id);
    }

    pub fn reserved_purchase_price(
        &mut self,
        player_id: &str,
        parcel_id: &str,
    ) -> PropertyResult<u64> {
        self.service.reserved_purchase_price(player_id, parcel_id)
    }

    pub fn complete_sandbox_purchase(&mut self, player_id: &str, parcel_id: &str) -> bool {
        self.service.complete_sandbox_purchase(player_id, parcel_id)
    }
}

pub struct PropertyService<P: BlockchainProvider> {
    provider: Arc<P>,
    wallets: PlayerWalletRegistry,
    parcels: HashMap<String, PropertyParcel>,
    protected_zones: Vec<ProtectedZone>,
    building_assets: HashMap<String, BuildingAsset>,
    placements: HashMap<String, Vec<PlacedBuilding>>,
    reservations: HashMap<String, (String, Instant)>,
    persistence_path: Option<PathBuf>,
}

impl<P: BlockchainProvider> PropertyService<P> {
    pub fn sandbox_assign_land(&mut self, wallet: &str, land_nft_id: &str) -> bool {
        Arc::get_mut(&mut self.provider)
            .is_some_and(|provider| provider.sandbox_assign_land(wallet, land_nft_id))
    }

    pub fn parcel_infos(&self) -> Vec<common_net::msg::PropertyParcelInfo> {
        self.parcel_infos_for_player(None)
    }

    pub fn parcel_infos_for_player(
        &self,
        player_id: Option<&str>,
    ) -> Vec<common_net::msg::PropertyParcelInfo> {
        self.parcels
            .values()
            .map(|parcel| {
                let placed_positions = self
                    .placements
                    .get(&parcel.id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|position| common_net::msg::PropertyPlacementLocation {
                        x: position.x,
                        y: position.y,
                    })
                    .collect();
                common_net::msg::PropertyParcelInfo {
                    id: parcel.id.clone(),
                    name: format!("{} Land {}", parcel.land_type, parcel.id),
                    world_id: parcel.world_id.clone(),
                    continent: parcel.continent.clone(),
                    region: parcel.region.clone(),
                    land_type: parcel.land_type.clone(),
                    rarity: parcel.rarity.clone(),
                    terrain_type: parcel.terrain_type.clone(),
                    water_access: parcel.water_access,
                    road_access: parcel.road_access,
                    size_label: if parcel.max_buildings >= 20 {
                        "Vast".to_string()
                    } else if parcel.max_buildings >= 10 {
                        "Large".to_string()
                    } else if parcel.max_buildings >= 5 {
                        "Medium".to_string()
                    } else {
                        "Small".to_string()
                    },
                    price_lamports: 0,
                    price_vgld_base_units: parcel.price_vgld_base_units,
                    status: if self.parcel_is_protected(parcel) {
                        "PROTECTED".to_string()
                    } else if self.parcel_is_reserved(&parcel.id) {
                        "RESERVED".to_string()
                    } else if player_id
                        .and_then(|player_id| self.wallets.linked_wallet(player_id))
                        .is_some_and(|wallet| {
                            self.provider
                                .verify_land_ownership(wallet, &parcel.land_nft_id, "land")
                        })
                    {
                        "OWNED".to_string()
                    } else {
                        "AVAILABLE".to_string()
                    },
                    protected: self.parcel_is_protected(parcel),
                    is_owned: player_id
                        .and_then(|player_id| self.wallets.linked_wallet(player_id))
                        .is_some_and(|wallet| {
                            self.provider
                                .verify_land_ownership(wallet, &parcel.land_nft_id, "land")
                        }),
                    min_x: parcel.bounds.min_x,
                    min_y: parcel.bounds.min_y,
                    max_x: parcel.bounds.max_x,
                    max_y: parcel.bounds.max_y,
                    allowed_buildings: parcel.allowed_buildings.clone(),
                    max_buildings: parcel.max_buildings as u32,
                    placed_buildings: self
                        .placements
                        .get(&parcel.id)
                        .map_or(0, |placements| placements.len() as u32),
                    placed_positions,
                }
            })
            .collect()
    }

    fn building_footprint_for_asset(&self, building: &BuildingAsset) -> BuildingFootprint {
        building.footprint.clone()
    }

    fn placement_conflicts_with_existing(
        &self,
        parcel: &PropertyParcel,
        placement: &WorldPosition,
        footprint: &BuildingFootprint,
    ) -> bool {
        let Some(existing) = self.placements.get(&parcel.id) else {
            return false;
        };

        existing.iter().any(|placed| {
            let existing_footprint = placed.footprint.clone();
            footprint.overlaps_blocked_area(
                &existing_footprint,
                placement.x,
                placement.y,
                placed.x,
                placed.y,
            )
        })
    }

    pub fn new(provider: Arc<P>) -> Self {
        Self {
            provider,
            wallets: PlayerWalletRegistry::new().with_challenge_ttl(Duration::from_secs(300)),
            parcels: HashMap::new(),
            protected_zones: Vec::new(),
            building_assets: HashMap::new(),
            placements: HashMap::new(),
            reservations: HashMap::new(),
            persistence_path: None,
        }
    }

    pub fn set_persistence_path(&mut self, path: PathBuf) { self.persistence_path = Some(path); }

    pub fn load_placements(&mut self) {
        let Some(path) = self.persistence_path.as_deref() else {
            return;
        };
        let Ok(bytes) = fs::read(path) else {
            return;
        };

        match serde_json::from_slice::<HashMap<String, Vec<PlacedBuilding>>>(&bytes) {
            Ok(placements) => self.placements = placements,
            Err(_) => match serde_json::from_slice::<HashMap<String, Vec<WorldPosition>>>(&bytes) {
                Ok(legacy) => {
                    self.placements = legacy
                        .into_iter()
                        .map(|(parcel_id, positions)| {
                            (
                                parcel_id,
                                positions
                                    .into_iter()
                                    .map(|position| PlacedBuilding {
                                        x: position.x,
                                        y: position.y,
                                        building_type: "legacy".to_string(),
                                        footprint: BuildingFootprint::small(),
                                    })
                                    .collect(),
                            )
                        })
                        .collect();
                },
                Err(err) => tracing::warn!(?path, ?err, "Ignoring invalid property placement data"),
            },
        }
    }

    fn save_placements(
        &self,
        placements: &HashMap<String, Vec<PlacedBuilding>>,
    ) -> PropertyResult<()> {
        let Some(path) = self.persistence_path.as_deref() else {
            return Ok(());
        };
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|_| PropertyError::PersistenceFailed)?;
        let bytes = serde_json::to_vec(placements).map_err(|_| PropertyError::PersistenceFailed)?;
        let temporary_path = path.with_extension("tmp");
        fs::write(&temporary_path, bytes).map_err(|_| PropertyError::PersistenceFailed)?;
        fs::rename(&temporary_path, path).map_err(|_| PropertyError::PersistenceFailed)?;
        Ok(())
    }

    pub fn register_parcel(&mut self, parcel: PropertyParcel) {
        self.parcels.insert(parcel.id.clone(), parcel);
    }

    pub fn reserve_purchase(&mut self, player_id: &str, parcel_id: &str) -> PropertyResult<u64> {
        self.reservations
            .retain(|_, (_, created)| created.elapsed() < Duration::from_secs(300));
        let parcel = self
            .parcels
            .get(parcel_id)
            .ok_or(PropertyError::ParcelNotFound)?;
        if self.parcel_is_protected(parcel) {
            return Err(PropertyError::ProtectedZone);
        }
        if self.reservations.contains_key(parcel_id) {
            return Err(PropertyError::PurchaseReserved);
        }
        let wallet = self
            .wallets
            .linked_wallet(player_id)
            .ok_or(PropertyError::WalletNotLinked)?;
        if self
            .provider
            .verify_land_ownership(wallet, &parcel.land_nft_id, "land")
        {
            return Err(PropertyError::LandOwnershipMissing);
        }
        self.reservations.insert(
            parcel_id.to_string(),
            (player_id.to_string(), Instant::now()),
        );
        Ok(parcel.price_vgld_base_units)
    }

    pub fn release_purchase(&mut self, parcel_id: &str) { self.reservations.remove(parcel_id); }

    pub fn reserved_purchase_price(
        &mut self,
        player_id: &str,
        parcel_id: &str,
    ) -> PropertyResult<u64> {
        self.reservations
            .retain(|_, (_, created)| created.elapsed() < Duration::from_secs(300));
        let Some((reserved_player, _)) = self.reservations.get(parcel_id) else {
            return Err(PropertyError::PurchaseReserved);
        };
        if reserved_player != player_id {
            return Err(PropertyError::PurchaseReserved);
        }
        self.parcels
            .get(parcel_id)
            .map(|parcel| parcel.price_vgld_base_units)
            .ok_or(PropertyError::ParcelNotFound)
    }

    pub fn complete_sandbox_purchase(&mut self, player_id: &str, parcel_id: &str) -> bool {
        let Ok(_) = self.reserved_purchase_price(player_id, parcel_id) else {
            return false;
        };
        let Some(wallet) = self.wallets.linked_wallet(player_id).map(str::to_owned) else {
            return false;
        };
        let Some(land_nft_id) = self
            .parcels
            .get(parcel_id)
            .map(|parcel| parcel.land_nft_id.clone())
        else {
            return false;
        };
        if !self.sandbox_assign_land(&wallet, &land_nft_id) {
            return false;
        }
        self.reservations.remove(parcel_id);
        true
    }

    fn parcel_is_reserved(&self, parcel_id: &str) -> bool {
        self.reservations.contains_key(parcel_id)
    }

    pub fn register_protected_zone(&mut self, zone: ProtectedZone) {
        self.protected_zones.push(zone);
    }

    fn parcel_is_protected(&self, parcel: &PropertyParcel) -> bool {
        parcel.protected
            || self.protected_zones.iter().any(|zone| {
                zone.world_id == parcel.world_id
                    && zone.bounds.min_x <= parcel.bounds.max_x
                    && zone.bounds.max_x >= parcel.bounds.min_x
                    && zone.bounds.min_y <= parcel.bounds.max_y
                    && zone.bounds.max_y >= parcel.bounds.min_y
            })
    }

    pub fn register_default_development_parcels(&mut self) {
        let farm_land_nft_id =
            std::env::var("VELOREN_MARKETPLACE_LAND_MINT").unwrap_or_else(|_| "land-2".to_string());
        for (id, min_x, min_y, max_x, max_y, reason) in [
            (
                "PROTECTED-C1-CAPITAL",
                -640,
                -640,
                -320,
                -320,
                "capital city",
            ),
            ("PROTECTED-C2-CAPITAL", -160, -160, 160, 160, "capital city"),
            ("PROTECTED-C3-CAPITAL", 320, 320, 640, 640, "capital city"),
        ] {
            self.register_protected_zone(ProtectedZone {
                id: id.to_string(),
                world_id: "WORLD-001".to_string(),
                bounds: ParcelBounds {
                    min_x,
                    min_y,
                    max_x,
                    max_y,
                },
                reason: reason.to_string(),
            });
        }
        let default_parcels = [
            PropertyParcel {
                id: "DEV-LAND-0001".to_string(),
                land_nft_id: "land-1".to_string(),
                world_id: "WORLD-001".to_string(),
                continent: "continent-1".to_string(),
                region: "frontier-town".to_string(),
                land_type: "town".to_string(),
                rarity: "rare".to_string(),
                terrain_type: "forest edge".to_string(),
                water_access: true,
                road_access: true,
                price_vgld_base_units: 2_500 * 1_000_000_000,
                protected: false,
                bounds: ParcelBounds {
                    min_x: -32,
                    min_y: -32,
                    max_x: 192,
                    max_y: 192,
                },
                allowed_buildings: vec![
                    "house".to_string(),
                    "shop".to_string(),
                    "inn".to_string(),
                    "blacksmith".to_string(),
                    "guild_hall".to_string(),
                ],
                max_buildings: 12,
            },
            PropertyParcel {
                id: "DEV-LAND-0002".to_string(),
                land_nft_id: farm_land_nft_id,
                world_id: "WORLD-001".to_string(),
                continent: "continent-2".to_string(),
                region: "silver-river-valley".to_string(),
                land_type: "farm".to_string(),
                rarity: "uncommon".to_string(),
                terrain_type: "river valley".to_string(),
                water_access: true,
                road_access: true,
                price_vgld_base_units: 800 * 1_000_000_000,
                protected: false,
                bounds: ParcelBounds {
                    min_x: 200,
                    min_y: 80,
                    max_x: 380,
                    max_y: 240,
                },
                allowed_buildings: vec![
                    "farmhouse".to_string(),
                    "barn".to_string(),
                    "stable".to_string(),
                ],
                max_buildings: 8,
            },
            PropertyParcel {
                id: "DEV-LAND-0003".to_string(),
                land_nft_id: "land-3".to_string(),
                world_id: "WORLD-001".to_string(),
                continent: "continent-3".to_string(),
                region: "highland-citadel".to_string(),
                land_type: "citadel".to_string(),
                rarity: "epic".to_string(),
                terrain_type: "mountain valley".to_string(),
                water_access: false,
                road_access: false,
                price_vgld_base_units: 10_000 * 1_000_000_000,
                protected: false,
                bounds: ParcelBounds {
                    min_x: -300,
                    min_y: -220,
                    max_x: -20,
                    max_y: 40,
                },
                allowed_buildings: vec![
                    "castle".to_string(),
                    "fortress".to_string(),
                    "town_hall".to_string(),
                ],
                max_buildings: 6,
            },
        ];

        for parcel in default_parcels {
            self.register_parcel(parcel);
        }
    }

    fn load_or_generate_parcels(&mut self, path: PathBuf) {
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(parcels) = serde_json::from_slice::<Vec<PropertyParcel>>(&bytes) {
                for parcel in parcels {
                    self.register_parcel(parcel);
                }
                return;
            }
        }

        self.register_generated_parcels();
        let parcels = self
            .parcels
            .values()
            .filter(|parcel| !parcel.id.starts_with("DEV-"))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temporary_path = path.with_extension("tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(&parcels) {
            if fs::write(&temporary_path, bytes)
                .and_then(|_| fs::rename(&temporary_path, &path))
                .is_err()
            {
                tracing::warn!(?path, "Unable to persist generated parcel catalog");
            }
        }
    }

    fn register_generated_parcels(&mut self) {
        let locations = [
            ("C1", "continent-1", "western frontier", -900, -180),
            ("C1", "continent-1", "western forest edge", -760, 420),
            ("C1", "continent-1", "western foothills", -520, -520),
            ("C2", "continent-2", "central river valley", -120, 520),
            ("C2", "continent-2", "central high plains", 180, -520),
            ("C2", "continent-2", "central lakeside", 520, 260),
            ("C3", "continent-3", "eastern desert edge", 760, -260),
            ("C3", "continent-3", "eastern mountain valley", 900, 460),
            ("C3", "continent-3", "eastern coastal plain", 520, -700),
        ];
        let types = [
            ("homestead", "common", 2_usize, 96, 96, 150_u64),
            ("farm", "uncommon", 5, 160, 128, 800),
            ("ranch", "rare", 7, 224, 176, 1400),
        ];

        for (index, (continent_id, continent, region, center_x, center_y)) in
            locations.into_iter().enumerate()
        {
            for (type_index, (land_type, rarity, capacity, width, height, price_vgld)) in
                types.into_iter().enumerate()
            {
                let id = format!("LAND-WORLD001-{continent_id}-{index:02}{type_index:02}");
                let min_x = center_x + type_index as i32 * 280 - width / 2;
                let min_y = center_y + type_index as i32 * 220 - height / 2;
                self.register_parcel(PropertyParcel {
                    id,
                    land_nft_id: format!("unminted-land-{index:02}-{type_index:02}"),
                    world_id: "WORLD-001".to_string(),
                    continent: continent.to_string(),
                    region: region.to_string(),
                    land_type: land_type.to_string(),
                    rarity: rarity.to_string(),
                    terrain_type: match type_index {
                        0 => "plains",
                        1 => "farmland",
                        _ => "high plains",
                    }
                    .to_string(),
                    water_access: type_index != 1,
                    road_access: type_index == 1,
                    price_vgld_base_units: price_vgld * 1_000_000_000,
                    protected: false,
                    bounds: ParcelBounds {
                        min_x,
                        min_y,
                        max_x: min_x + width,
                        max_y: min_y + height,
                    },
                    allowed_buildings: vec![
                        "house".to_string(),
                        "farmhouse".to_string(),
                        "barn".to_string(),
                        "stable".to_string(),
                    ],
                    max_buildings: capacity,
                });
            }
        }
    }

    pub fn register_building_asset(&mut self, asset: BuildingAsset) {
        self.building_assets.insert(asset.id.clone(), asset);
    }

    pub fn register_default_development_buildings(&mut self) {
        let default_assets = [
            BuildingAsset {
                id: "building.house.v1".to_string(),
                building_type: "house".to_string(),
                game_asset_id: "building.house.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::medium(),
            },
            BuildingAsset {
                id: "building.shop.v1".to_string(),
                building_type: "shop".to_string(),
                game_asset_id: "building.shop.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::large(),
            },
            BuildingAsset {
                id: "building.inn.v1".to_string(),
                building_type: "inn".to_string(),
                game_asset_id: "building.inn.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::large(),
            },
            BuildingAsset {
                id: "building.blacksmith.v1".to_string(),
                building_type: "blacksmith".to_string(),
                game_asset_id: "building.blacksmith.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::large(),
            },
            BuildingAsset {
                id: "building.guild_hall.v1".to_string(),
                building_type: "guild_hall".to_string(),
                game_asset_id: "building.guild_hall.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::very_large(),
            },
            BuildingAsset {
                id: "building.farmhouse.v1".to_string(),
                building_type: "farmhouse".to_string(),
                game_asset_id: "building.farmhouse.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::large(),
            },
            BuildingAsset {
                id: "building.barn.v1".to_string(),
                building_type: "barn".to_string(),
                game_asset_id: "building.barn.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::large(),
            },
            BuildingAsset {
                id: "building.stable.v1".to_string(),
                building_type: "stable".to_string(),
                game_asset_id: "building.stable.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::large(),
            },
            BuildingAsset {
                id: "building.castle.v1".to_string(),
                building_type: "castle".to_string(),
                game_asset_id: "building.castle.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::very_large(),
            },
            BuildingAsset {
                id: "building.fortress.v1".to_string(),
                building_type: "fortress".to_string(),
                game_asset_id: "building.fortress.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::very_large(),
            },
            BuildingAsset {
                id: "building.town_hall.v1".to_string(),
                building_type: "town_hall".to_string(),
                game_asset_id: "building.town_hall.v1".to_string(),
                collection: "building".to_string(),
                footprint: BuildingFootprint::very_large(),
            },
        ];

        for asset in default_assets {
            self.register_building_asset(asset);
        }
    }

    pub fn issue_wallet_challenge(&mut self, player_id: &str) -> String {
        self.wallets.issue_challenge(player_id)
    }

    pub fn link_wallet(
        &mut self,
        player_id: &str,
        wallet: &str,
        challenge: &str,
        signature: &str,
    ) -> PropertyResult<()> {
        self.wallets
            .link_wallet(player_id, wallet, challenge, signature)
    }

    pub fn building_type_for_placement(&self, building_nft_id: &str) -> Option<String> {
        let building_metadata = self.provider.get_building_metadata(building_nft_id)?;
        let asset = self
            .building_assets
            .get(&building_metadata.token_id)
            .or_else(|| {
                self.building_assets
                    .values()
                    .find(|candidate| candidate.id == building_metadata.token_id)
            })
            .or_else(|| {
                building_metadata
                    .building_type
                    .as_deref()
                    .and_then(|building_type| {
                        self.building_assets
                            .values()
                            .find(|candidate| candidate.building_type == building_type)
                    })
            })?;
        Some(asset.building_type.clone())
    }

    pub fn authorize_placement(
        &mut self,
        player_id: &str,
        parcel_id: &str,
        land_nft_id: &str,
        building_nft_id: &str,
        placement: WorldPosition,
    ) -> PropertyResult<()> {
        let wallet = self
            .wallets
            .linked_wallet(player_id)
            .ok_or(PropertyError::WalletNotLinked)?;

        let parcel = self
            .parcels
            .get(parcel_id)
            .ok_or(PropertyError::ParcelNotFound)?;
        if self.parcel_is_protected(parcel) {
            return Err(PropertyError::ProtectedZone);
        }
        if !self
            .provider
            .verify_land_ownership(wallet, land_nft_id, "land")
        {
            return Err(PropertyError::LandOwnershipMissing);
        }
        if land_nft_id != parcel.land_nft_id {
            return Err(PropertyError::LandMetadataMismatch);
        }
        if let Some(land_metadata) = self.provider.get_land_metadata(land_nft_id)
            && (land_metadata.asset_type != "land"
                || land_metadata.land_type.as_deref().is_some_and(|land_type| land_type != parcel.land_type)
                || land_metadata.world_id.as_deref().is_some_and(|world_id| world_id != parcel.world_id))
        {
            return Err(PropertyError::LandMetadataMismatch);
        }

        let building_metadata = self
            .provider
            .get_building_metadata(building_nft_id)
            .ok_or(PropertyError::UnknownAsset)?;

        let asset = self
            .building_assets
            .get(&building_metadata.token_id)
            .or_else(|| {
                self.building_assets
                    .values()
                    .find(|candidate| candidate.id == building_metadata.token_id)
            })
            .or_else(|| {
                building_metadata
                    .building_type
                    .as_deref()
                    .and_then(|building_type| {
                        self.building_assets
                            .values()
                            .find(|candidate| candidate.building_type == building_type)
                    })
            })
            .ok_or(PropertyError::UnknownAsset)?;

        if !self
            .provider
            .verify_building_ownership(wallet, building_nft_id, "building")
        {
            return Err(PropertyError::BuildingOwnershipMissing);
        }

        if !parcel
            .allowed_buildings
            .iter()
            .any(|kind| kind == &asset.building_type)
        {
            return Err(PropertyError::BuildingTypeNotAllowed);
        }

        let footprint = self.building_footprint_for_asset(&asset);

        let min_x = placement.x - footprint.width / 2;
        let max_x = placement.x + footprint.width / 2;
        let min_y = placement.y - footprint.height / 2;
        let max_y = placement.y + footprint.height / 2;

        let in_bounds = min_x >= parcel.bounds.min_x
            && max_x <= parcel.bounds.max_x
            && min_y >= parcel.bounds.min_y
            && max_y <= parcel.bounds.max_y;

        if !in_bounds {
            return Err(PropertyError::BuildingPlacementOutOfBounds);
        }

        let placed = self.placements.entry(parcel_id.to_owned()).or_default();
        if placed.len() >= parcel.max_buildings {
            return Err(PropertyError::ParcelFull);
        }

        if self.placement_conflicts_with_existing(parcel, &placement, &footprint) {
            return Err(PropertyError::BuildingPlacementConflict);
        }

        let mut updated_placements = self.placements.clone();
        updated_placements
            .entry(parcel_id.to_owned())
            .or_default()
            .push(PlacedBuilding {
                x: placement.x,
                y: placement.y,
                building_type: asset.building_type.clone(),
                footprint: footprint.clone(),
            });
        self.save_placements(&updated_placements)?;
        self.placements = updated_placements;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn test_wallet() -> (String, SigningKey) {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        (
            bs58::encode(signing_key.verifying_key().to_bytes()).into_string(),
            signing_key,
        )
    }

    #[test]
    fn wallet_link_requires_valid_signature() {
        let mut registry = PlayerWalletRegistry::new();
        let challenge = registry.issue_challenge("player-1");
        let (wallet, signing_key) = test_wallet();
        let signature = bs58::encode(
            signing_key
                .sign(wallet_link_message(&wallet, &challenge).as_bytes())
                .to_bytes(),
        )
        .into_string();

        assert!(
            registry
                .link_wallet("player-1", &wallet, &challenge, &signature)
                .is_ok()
        );

        let challenge = registry.issue_challenge("player-2");
        assert!(
            registry
                .link_wallet("player-2", &wallet, &challenge, "not-a-signature")
                .is_err()
        );
    }

    #[test]
    fn property_service_rejects_unlinked_wallet() {
        let provider = Arc::new(MockBlockchainProvider::new());
        let mut service = PropertyService::new(provider);

        let parcel = PropertyParcel {
            id: "parcel-1".to_string(),
            land_nft_id: "land-1".to_string(),
            world_id: "world-1".to_string(),
            continent: "continent-test".to_string(),
            region: "region-test".to_string(),
            land_type: "town".to_string(),
            rarity: "common".to_string(),
            terrain_type: "plains".to_string(),
            water_access: false,
            road_access: false,
            price_vgld_base_units: 0,
            protected: false,
            bounds: ParcelBounds {
                min_x: 0,
                min_y: 0,
                max_x: 10,
                max_y: 10,
            },
            allowed_buildings: vec!["shop".to_string()],
            max_buildings: 1,
        };
        service.register_parcel(parcel);

        let err = service.authorize_placement(
            "player-1",
            "parcel-1",
            "land-1",
            "building-1",
            WorldPosition { x: 5, y: 5 },
        );

        assert_eq!(err, Err(PropertyError::WalletNotLinked));
    }

    #[test]
    fn wallet_registry_exposes_vgld_account_lookup() {
        let mut registry = PlayerWalletRegistry::new();
        let (wallet, signing_key) = test_wallet();
        let challenge = registry.issue_challenge("player-1");
        let signature = bs58::encode(
            signing_key
                .sign(wallet_link_message(&wallet, &challenge).as_bytes())
                .to_bytes(),
        )
        .into_string();

        registry
            .link_wallet("player-1", &wallet, &challenge, &signature)
            .unwrap();

        assert_eq!(
            registry.account("player-1"),
            Some(WalletAccount {
                player_id: "player-1".to_string(),
                wallet: wallet.clone(),
            })
        );
        assert_eq!(registry.player_for_wallet(&wallet), Some("player-1"));
    }

    #[test]
    fn property_service_allows_authorized_placement() {
        let mut provider = MockBlockchainProvider::new();
        let (wallet, signing_key) = test_wallet();

        provider.register_land_nft("land-1", &wallet, NftMetadata {
            token_id: "land-1".to_string(),
            collection: "land".to_string(),
            asset_type: "land".to_string(),
            land_type: Some("town".to_string()),
            building_type: None,
            world_id: Some("world-1".to_string()),
            metadata_uri: "ipfs://land-1".to_string(),
        });

        provider.register_building_nft("building-1", &wallet, NftMetadata {
            token_id: "building-1".to_string(),
            collection: "building".to_string(),
            asset_type: "building".to_string(),
            land_type: None,
            building_type: Some("shop".to_string()),
            world_id: None,
            metadata_uri: "ipfs://building-1".to_string(),
        });

        let provider = Arc::new(provider);
        let mut service = PropertyService::new(provider.clone());
        service.register_building_asset(BuildingAsset {
            id: "building-1".to_string(),
            building_type: "shop".to_string(),
            game_asset_id: "building.shop.large.v1".to_string(),
            collection: "building".to_string(),
            footprint: BuildingFootprint::large(),
        });

        let parcel = PropertyParcel {
            id: "parcel-1".to_string(),
            land_nft_id: "land-1".to_string(),
            world_id: "world-1".to_string(),
            continent: "continent-test".to_string(),
            region: "region-test".to_string(),
            land_type: "town".to_string(),
            rarity: "common".to_string(),
            terrain_type: "plains".to_string(),
            water_access: false,
            road_access: false,
            price_vgld_base_units: 0,
            protected: false,
            bounds: ParcelBounds {
                min_x: 0,
                min_y: 0,
                max_x: 80,
                max_y: 80,
            },
            allowed_buildings: vec!["shop".to_string()],
            max_buildings: 2,
        };
        service.register_parcel(parcel);

        let challenge = service.issue_wallet_challenge("player-1");
        service
            .link_wallet(
                "player-1",
                &wallet,
                &challenge,
                &bs58::encode(
                    signing_key
                        .sign(wallet_link_message(&wallet, &challenge).as_bytes())
                        .to_bytes(),
                )
                .into_string(),
            )
            .unwrap();

        let result = service.authorize_placement(
            "player-1",
            "parcel-1",
            "land-1",
            "building-1",
            WorldPosition { x: 12, y: 12 },
        );

        assert!(result.is_ok());
    }

    #[test]
    fn property_service_rejects_building_type_not_allowed() {
        let mut provider = MockBlockchainProvider::new();
        let (wallet, signing_key) = test_wallet();

        provider.register_land_nft("land-1", &wallet, NftMetadata {
            token_id: "land-1".to_string(),
            collection: "land".to_string(),
            asset_type: "land".to_string(),
            land_type: Some("town".to_string()),
            building_type: None,
            world_id: Some("world-1".to_string()),
            metadata_uri: "ipfs://land-1".to_string(),
        });

        provider.register_building_nft("building-1", &wallet, NftMetadata {
            token_id: "building-1".to_string(),
            collection: "building".to_string(),
            asset_type: "building".to_string(),
            land_type: None,
            building_type: Some("farmhouse".to_string()),
            world_id: None,
            metadata_uri: "ipfs://building-1".to_string(),
        });

        let provider = Arc::new(provider);
        let mut service = PropertyService::new(provider.clone());
        service.register_building_asset(BuildingAsset {
            id: "building-1".to_string(),
            building_type: "farmhouse".to_string(),
            game_asset_id: "building.farmhouse.v1".to_string(),
            collection: "building".to_string(),
            footprint: BuildingFootprint::large(),
        });

        service.register_parcel(PropertyParcel {
            id: "parcel-1".to_string(),
            land_nft_id: "land-1".to_string(),
            world_id: "world-1".to_string(),
            continent: "continent-test".to_string(),
            region: "region-test".to_string(),
            land_type: "town".to_string(),
            rarity: "common".to_string(),
            terrain_type: "plains".to_string(),
            water_access: false,
            road_access: false,
            price_vgld_base_units: 0,
            protected: false,
            bounds: ParcelBounds {
                min_x: 0,
                min_y: 0,
                max_x: 10,
                max_y: 10,
            },
            allowed_buildings: vec!["shop".to_string()],
            max_buildings: 1,
        });

        let challenge = service.issue_wallet_challenge("player-1");
        service
            .link_wallet(
                "player-1",
                &wallet,
                &challenge,
                &bs58::encode(
                    signing_key
                        .sign(wallet_link_message(&wallet, &challenge).as_bytes())
                        .to_bytes(),
                )
                .into_string(),
            )
            .unwrap();

        let result = service.authorize_placement(
            "player-1",
            "parcel-1",
            "land-1",
            "building-1",
            WorldPosition { x: 5, y: 5 },
        );

        assert_eq!(result, Err(PropertyError::BuildingTypeNotAllowed));
    }

    #[test]
    fn property_service_rejects_land_for_another_parcel() {
        let mut provider = MockBlockchainProvider::new();
        let (wallet, signing_key) = test_wallet();

        provider.register_land_nft("land-1", &wallet, NftMetadata {
            token_id: "land-1".to_string(),
            collection: "land".to_string(),
            asset_type: "land".to_string(),
            land_type: Some("farm".to_string()),
            building_type: None,
            world_id: Some("world-1".to_string()),
            metadata_uri: "ipfs://land-1".to_string(),
        });
        provider.register_building_nft("building-1", &wallet, NftMetadata {
            token_id: "building-1".to_string(),
            collection: "building".to_string(),
            asset_type: "building".to_string(),
            land_type: None,
            building_type: Some("shop".to_string()),
            world_id: None,
            metadata_uri: "ipfs://building-1".to_string(),
        });

        let provider = Arc::new(provider);
        let mut service = PropertyService::new(provider);
        service.register_building_asset(BuildingAsset {
            id: "building-1".to_string(),
            building_type: "shop".to_string(),
            game_asset_id: "building.shop.v1".to_string(),
            collection: "building".to_string(),
            footprint: BuildingFootprint::large(),
        });
        service.register_parcel(PropertyParcel {
            id: "parcel-1".to_string(),
            land_nft_id: "land-1".to_string(),
            world_id: "world-1".to_string(),
            continent: "continent-test".to_string(),
            region: "region-test".to_string(),
            land_type: "town".to_string(),
            rarity: "common".to_string(),
            terrain_type: "plains".to_string(),
            water_access: false,
            road_access: false,
            price_vgld_base_units: 0,
            protected: false,
            bounds: ParcelBounds {
                min_x: 0,
                min_y: 0,
                max_x: 10,
                max_y: 10,
            },
            allowed_buildings: vec!["shop".to_string()],
            max_buildings: 1,
        });

        let challenge = service.issue_wallet_challenge("player-1");
        service
            .link_wallet(
                "player-1",
                &wallet,
                &challenge,
                &bs58::encode(
                    signing_key
                        .sign(wallet_link_message(&wallet, &challenge).as_bytes())
                        .to_bytes(),
                )
                .into_string(),
            )
            .unwrap();

        assert_eq!(
            service.authorize_placement(
                "player-1",
                "parcel-1",
                "land-1",
                "building-1",
                WorldPosition { x: 5, y: 5 },
            ),
            Err(PropertyError::LandMetadataMismatch)
        );
    }

    #[test]
    fn development_parcel_registry_loads_default_parcels() {
        let service =
            PropertyService::<MockBlockchainProvider>::new(Arc::new(MockBlockchainProvider::new()));
        let mut service = service;

        service.register_default_development_parcels();
        service.register_default_development_buildings();

        assert!(service.parcels.contains_key("DEV-LAND-0001"));
        assert!(service.parcels.contains_key("DEV-LAND-0002"));
        assert!(service.parcels.contains_key("DEV-LAND-0003"));
        assert!(service.building_assets.contains_key("building.house.v1"));

        let town = service.parcels.get("DEV-LAND-0001").unwrap();
        assert_eq!(town.land_type, "town");
        assert!(town.bounds.max_x - town.bounds.min_x >= 128);
        assert!(town.max_buildings >= 12);
    }

    #[test]
    fn property_service_reserves_available_parcel_once() {
        let mut service = PropertyService::new(Arc::new(MockBlockchainProvider::new()));
        service.register_parcel(PropertyParcel {
            id: "parcel-1".to_string(),
            land_nft_id: "land-1".to_string(),
            world_id: "world-1".to_string(),
            continent: "continent-test".to_string(),
            region: "region-test".to_string(),
            land_type: "farm".to_string(),
            rarity: "common".to_string(),
            terrain_type: "plains".to_string(),
            water_access: false,
            road_access: true,
            price_vgld_base_units: 800,
            protected: false,
            bounds: ParcelBounds {
                min_x: 0,
                min_y: 0,
                max_x: 100,
                max_y: 100,
            },
            allowed_buildings: vec!["farmhouse".to_string()],
            max_buildings: 5,
        });
        service
            .wallets
            .linked_wallets
            .insert("player-1".to_string(), "wallet-1".to_string());

        assert_eq!(service.reserve_purchase("player-1", "parcel-1"), Ok(800));
        assert_eq!(
            service.reserve_purchase("player-2", "parcel-1"),
            Err(PropertyError::PurchaseReserved)
        );
        service.release_purchase("parcel-1");
        service
            .wallets
            .linked_wallets
            .insert("player-2".to_string(), "wallet-2".to_string());
        assert_eq!(service.reserve_purchase("player-2", "parcel-1"), Ok(800));
    }

    #[test]
    fn property_service_rejects_overlapping_buildings() {
        let mut provider = MockBlockchainProvider::new();
        let (wallet, signing_key) = test_wallet();

        provider.register_land_nft("land-1", &wallet, NftMetadata {
            token_id: "land-1".to_string(),
            collection: "land".to_string(),
            asset_type: "land".to_string(),
            land_type: Some("town".to_string()),
            building_type: None,
            world_id: Some("world-1".to_string()),
            metadata_uri: "ipfs://land-1".to_string(),
        });

        provider.register_building_nft("building-1", &wallet, NftMetadata {
            token_id: "building-1".to_string(),
            collection: "building".to_string(),
            asset_type: "building".to_string(),
            land_type: None,
            building_type: Some("shop".to_string()),
            world_id: None,
            metadata_uri: "ipfs://building-1".to_string(),
        });

        let provider = Arc::new(provider);
        let mut service = PropertyService::new(provider);
        service.register_building_asset(BuildingAsset {
            id: "building-1".to_string(),
            building_type: "shop".to_string(),
            game_asset_id: "building.shop.v1".to_string(),
            collection: "building".to_string(),
            footprint: BuildingFootprint::large(),
        });
        service.register_parcel(PropertyParcel {
            id: "parcel-1".to_string(),
            land_nft_id: "land-1".to_string(),
            world_id: "world-1".to_string(),
            continent: "continent-test".to_string(),
            region: "region-test".to_string(),
            land_type: "town".to_string(),
            rarity: "common".to_string(),
            terrain_type: "plains".to_string(),
            water_access: false,
            road_access: false,
            price_vgld_base_units: 0,
            protected: false,
            bounds: ParcelBounds {
                min_x: 0,
                min_y: 0,
                max_x: 64,
                max_y: 64,
            },
            allowed_buildings: vec!["shop".to_string()],
            max_buildings: 2,
        });

        let challenge = service.issue_wallet_challenge("player-1");
        service
            .link_wallet(
                "player-1",
                &wallet,
                &challenge,
                &bs58::encode(
                    signing_key
                        .sign(wallet_link_message(&wallet, &challenge).as_bytes())
                        .to_bytes(),
                )
                .into_string(),
            )
            .unwrap();

        assert!(
            service
                .authorize_placement(
                    "player-1",
                    "parcel-1",
                    "land-1",
                    "building-1",
                    WorldPosition { x: 12, y: 12 },
                )
                .is_ok()
        );

        assert_eq!(
            service.authorize_placement(
                "player-1",
                "parcel-1",
                "land-1",
                "building-1",
                WorldPosition { x: 22, y: 22 },
            ),
            Err(PropertyError::BuildingPlacementConflict)
        );
    }
}
