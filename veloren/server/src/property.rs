//! Server-authoritative NFT property permissions.
//!
//! This module deliberately keeps blockchain access behind
//! [`BlockchainProvider`]. Gameplay code never trusts a wallet or token
//! identifier supplied by a client; it asks this service to verify the asset
//! and then applies the result locally.

use async_trait::async_trait;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, RwLock},
};

pub type WalletAddress = String;
pub type TokenId = String;
pub type CollectionId = String;
pub type LandId = String;
pub type BuildingId = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockchainNetwork {
    SolanaDevnet,
    SolanaTestnet,
    SolanaMainnet,
}

pub fn wallet_association_message(player_id: &str, challenge: &str) -> String {
    format!("Veloren wallet association\nplayer:{player_id}\nchallenge:{challenge}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockchainConfig {
    pub network: BlockchainNetwork,
    pub rpc_endpoint: String,
    pub land_collection: CollectionId,
    pub building_collection: CollectionId,
}

impl Default for BlockchainConfig {
    fn default() -> Self {
        Self {
            network: BlockchainNetwork::SolanaDevnet,
            rpc_endpoint: "https://api.devnet.solana.com".to_string(),
            land_collection: "land-devnet".to_string(),
            building_collection: "building-devnet".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    Land,
    Building,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NftMetadata {
    pub asset_kind: AssetKind,
    pub asset_id: String,
    pub asset_type: String,
    pub asset_version: u32,
    pub game_asset_id: Option<String>,
    pub allowed_buildings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NftAsset {
    pub token_id: TokenId,
    pub collection: CollectionId,
    pub owner: WalletAddress,
    pub metadata: NftMetadata,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BlockchainError {
    Unavailable(String),
    InvalidAsset(String),
}

impl fmt::Display for BlockchainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) => write!(f, "blockchain unavailable: {message}"),
            Self::InvalidAsset(message) => write!(f, "invalid blockchain asset: {message}"),
        }
    }
}

impl std::error::Error for BlockchainError {}

#[async_trait]
pub trait BlockchainProvider: Send + Sync {
    async fn get_asset(&self, token_id: &str) -> Result<Option<NftAsset>, BlockchainError>;

    async fn get_assets_by_owner(
        &self,
        wallet: &str,
        collection: &str,
    ) -> Result<Vec<NftAsset>, BlockchainError>;

    async fn verify_ownership(
        &self,
        wallet: &str,
        token_id: &str,
        collection: &str,
        kind: AssetKind,
    ) -> Result<NftAsset, BlockchainError> {
        let asset = self
            .get_asset(token_id)
            .await?
            .ok_or_else(|| BlockchainError::InvalidAsset("token does not exist".to_string()))?;

        if asset.owner != wallet
            || asset.collection != collection
            || asset.metadata.asset_kind != kind
        {
            return Err(BlockchainError::InvalidAsset(
                "ownership, collection, or asset kind verification failed".to_string(),
            ));
        }

        Ok(asset)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandParcel {
    pub land_id: LandId,
    pub token_id: TokenId,
    pub world_id: String,
    pub origin: (i32, i32),
    pub size: (u32, u32),
    pub land_type: String,
    pub allowed_buildings: Vec<String>,
    pub max_buildings: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildingPlacement {
    pub building_id: BuildingId,
    pub token_id: TokenId,
    pub land_id: LandId,
    pub position: (i32, i32, i32),
    pub rotation: f32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PropertyError {
    Blockchain(BlockchainError),
    UnknownLand(LandId),
    UnknownWallet,
    WalletChallengeMissing,
    InvalidWalletSignature,
    WalletMismatch,
    NotLandOwner,
    InvalidBuilding,
    BuildingNotAllowed,
    ParcelLimitReached,
    OutsideParcel,
    AlreadyPlaced,
}

impl fmt::Display for PropertyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blockchain(error) => error.fmt(f),
            Self::UnknownLand(id) => write!(f, "unknown land parcel {id}"),
            Self::UnknownWallet => write!(f, "player wallet is not linked to this account"),
            Self::WalletChallengeMissing => write!(f, "wallet association challenge is missing"),
            Self::InvalidWalletSignature => write!(f, "wallet signature is invalid"),
            Self::WalletMismatch => write!(f, "player wallet does not match the linked wallet"),
            Self::NotLandOwner => write!(f, "wallet does not own the land parcel"),
            Self::InvalidBuilding => write!(f, "building NFT is invalid"),
            Self::BuildingNotAllowed => write!(f, "building is not allowed on this parcel"),
            Self::ParcelLimitReached => write!(f, "parcel building limit reached"),
            Self::OutsideParcel => write!(f, "building is outside the parcel"),
            Self::AlreadyPlaced => write!(f, "building NFT is already placed"),
        }
    }
}

impl std::error::Error for PropertyError {}

impl From<BlockchainError> for PropertyError {
    fn from(error: BlockchainError) -> Self { Self::Blockchain(error) }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerWalletRegistry {
    wallets: HashMap<String, WalletAddress>,
    challenges: HashMap<String, String>,
}

impl PlayerWalletRegistry {
    pub fn issue_challenge(&mut self, player_id: &str, challenge: impl Into<String>) {
        self.challenges
            .insert(player_id.to_string(), challenge.into());
    }

    fn associate_wallet(&mut self, player_id: &str, wallet: &str) {
        self.wallets
            .insert(player_id.to_string(), wallet.to_string());
    }

    /// Associates a wallet only after verifying a one-time server challenge.
    ///
    /// Solana public keys and signatures are base58 encoded. The signed
    /// message is deliberately bound to the authenticated player id so a
    /// signature cannot be replayed for another account.
    pub fn associate_wallet_signed(
        &mut self,
        player_id: &str,
        wallet: &str,
        signature: &str,
    ) -> Result<(), PropertyError> {
        let challenge = self
            .challenges
            .get(player_id)
            .cloned()
            .ok_or(PropertyError::WalletChallengeMissing)?;
        let public_key_bytes = bs58::decode(wallet)
            .into_vec()
            .map_err(|_| PropertyError::InvalidWalletSignature)?;
        let public_key: [u8; 32] = public_key_bytes
            .try_into()
            .map_err(|_| PropertyError::InvalidWalletSignature)?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| PropertyError::InvalidWalletSignature)?;
        let signature_bytes = bs58::decode(signature)
            .into_vec()
            .map_err(|_| PropertyError::InvalidWalletSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| PropertyError::InvalidWalletSignature)?;
        let message = wallet_association_message(player_id, &challenge);
        verifying_key
            .verify(message.as_bytes(), &signature)
            .map_err(|_| PropertyError::InvalidWalletSignature)?;
        self.challenges.remove(player_id);
        self.associate_wallet(player_id, wallet);
        Ok(())
    }

    pub fn wallet_for_player(&self, player_id: &str) -> Option<&str> {
        self.wallets.get(player_id).map(String::as_str)
    }

    pub fn verify_player_wallet(&self, player_id: &str, wallet: &str) -> bool {
        self.wallets
            .get(player_id)
            .is_some_and(|registered_wallet| registered_wallet == wallet)
    }
}

/// Server-owned property state used by gameplay systems.
///
/// The provider is kept behind the same service used by tests, so replacing
/// the development provider with Solana does not change the authorization
/// boundary.
pub type DevelopmentPropertyService = PropertyService<MockBlockchainProvider>;

pub struct PropertyRuntime {
    pub wallets: Arc<RwLock<PlayerWalletRegistry>>,
    pub service: Arc<parking_lot::Mutex<DevelopmentPropertyService>>,
}

impl Default for PropertyRuntime {
    fn default() -> Self {
        let provider = Arc::new(MockBlockchainProvider::default());
        Self {
            wallets: Arc::new(RwLock::new(PlayerWalletRegistry::default())),
            service: Arc::new(parking_lot::Mutex::new(PropertyService::new(
                provider,
                BlockchainConfig::default(),
            ))),
        }
    }
}

impl PropertyRuntime {
    pub fn authorize_placement(
        &self,
        player_id: &str,
        land_id: &str,
        building_token_id: &str,
        position: (i32, i32, i32),
        rotation: f32,
    ) -> Result<BuildingPlacement, PropertyError> {
        let wallets = self
            .wallets
            .read()
            .expect("property wallet lock poisoned")
            .clone();
        futures::executor::block_on(self.service.lock().authorize_placement_for_player(
            player_id,
            &wallets,
            land_id,
            building_token_id,
            position,
            rotation,
        ))
    }
}

pub struct PropertyService<P> {
    provider: Arc<P>,
    config: BlockchainConfig,
    parcels: HashMap<LandId, LandParcel>,
    placements: HashMap<BuildingId, BuildingPlacement>,
}

impl<P: BlockchainProvider> PropertyService<P> {
    pub fn new(provider: Arc<P>, config: BlockchainConfig) -> Self {
        Self {
            provider,
            config,
            parcels: HashMap::new(),
            placements: HashMap::new(),
        }
    }

    pub fn register_parcel(&mut self, parcel: LandParcel) {
        self.parcels.insert(parcel.land_id.clone(), parcel);
    }

    pub async fn authorize_placement_for_player(
        &mut self,
        player_id: &str,
        wallet_registry: &PlayerWalletRegistry,
        land_id: &str,
        building_token_id: &str,
        position: (i32, i32, i32),
        rotation: f32,
    ) -> Result<BuildingPlacement, PropertyError> {
        let wallet = wallet_registry
            .wallet_for_player(player_id)
            .ok_or(PropertyError::UnknownWallet)?;

        self.place_building(wallet, land_id, building_token_id, position, rotation)
            .await
    }

    pub async fn place_building(
        &mut self,
        wallet: &str,
        land_id: &str,
        building_token_id: &str,
        position: (i32, i32, i32),
        rotation: f32,
    ) -> Result<BuildingPlacement, PropertyError> {
        let parcel = self
            .parcels
            .get(land_id)
            .ok_or_else(|| PropertyError::UnknownLand(land_id.to_string()))?;
        let land = self
            .provider
            .verify_ownership(
                wallet,
                &parcel.token_id,
                &self.config.land_collection,
                AssetKind::Land,
            )
            .await?;
        if land.metadata.asset_id != parcel.land_id {
            return Err(PropertyError::InvalidBuilding);
        }

        let building = self
            .provider
            .verify_ownership(
                wallet,
                building_token_id,
                &self.config.building_collection,
                AssetKind::Building,
            )
            .await?;

        let building_id = building.metadata.asset_id.clone();
        if self.placements.contains_key(&building_id) {
            return Err(PropertyError::AlreadyPlaced);
        }

        if building.metadata.game_asset_id.is_none() {
            return Err(PropertyError::InvalidBuilding);
        }

        let building_type = building.metadata.asset_type;
        if !parcel
            .allowed_buildings
            .iter()
            .any(|kind| kind == &building_type)
        {
            return Err(PropertyError::BuildingNotAllowed);
        }

        if self
            .placements
            .values()
            .filter(|placement| placement.land_id == land_id)
            .count() as u32
            >= parcel.max_buildings
        {
            return Err(PropertyError::ParcelLimitReached);
        }

        let inside_x =
            position.0 >= parcel.origin.0 && position.0 < parcel.origin.0 + parcel.size.0 as i32;
        let inside_y =
            position.1 >= parcel.origin.1 && position.1 < parcel.origin.1 + parcel.size.1 as i32;
        if !inside_x || !inside_y {
            return Err(PropertyError::OutsideParcel);
        }

        let placement = BuildingPlacement {
            building_id: building_id.clone(),
            token_id: building_token_id.to_string(),
            land_id: land_id.to_string(),
            position,
            rotation,
        };

        self.placements.insert(building_id, placement.clone());
        Ok(placement)
    }
}

#[derive(Clone, Default)]
pub struct MockBlockchainProvider {
    assets: Arc<RwLock<HashMap<TokenId, NftAsset>>>,
}

impl MockBlockchainProvider {
    pub fn insert(&self, asset: NftAsset) {
        self.assets
            .write()
            .expect("mock blockchain lock poisoned")
            .insert(asset.token_id.clone(), asset);
    }
}

#[async_trait]
impl BlockchainProvider for MockBlockchainProvider {
    async fn get_asset(&self, token_id: &str) -> Result<Option<NftAsset>, BlockchainError> {
        Ok(self
            .assets
            .read()
            .expect("mock blockchain lock poisoned")
            .get(token_id)
            .cloned())
    }

    async fn get_assets_by_owner(
        &self,
        wallet: &str,
        collection: &str,
    ) -> Result<Vec<NftAsset>, BlockchainError> {
        Ok(self
            .assets
            .read()
            .expect("mock blockchain lock poisoned")
            .values()
            .filter(|asset| asset.owner == wallet && asset.collection == collection)
            .cloned()
            .collect())
    }
}

#[derive(Clone, Debug)]
pub struct SolanaBlockchainProvider {
    client: reqwest::Client,
    rpc_endpoint: String,
}

impl SolanaBlockchainProvider {
    pub fn new(rpc_endpoint: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_endpoint: rpc_endpoint.into(),
        }
    }

    pub fn from_config(config: &BlockchainConfig) -> Self { Self::new(config.rpc_endpoint.clone()) }

    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, BlockchainError> {
        let response = self
            .client
            .post(&self.rpc_endpoint)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params
            }))
            .send()
            .await
            .map_err(|error| BlockchainError::Unavailable(error.to_string()))?;

        if !response.status().is_success() {
            return Err(BlockchainError::Unavailable(format!(
                "RPC returned HTTP {}",
                response.status()
            )));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| BlockchainError::Unavailable(error.to_string()))?;

        if let Some(error) = body.get("error") {
            return Err(BlockchainError::Unavailable(error.to_string()));
        }

        body.get("result")
            .cloned()
            .ok_or_else(|| BlockchainError::Unavailable("RPC result is missing".to_string()))
    }

    fn parse_asset(value: &serde_json::Value) -> Result<Option<NftAsset>, BlockchainError> {
        if value.is_null() {
            return Ok(None);
        }

        let token_id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("token_id").and_then(serde_json::Value::as_str))
            .ok_or_else(|| BlockchainError::InvalidAsset("asset id is missing".to_string()))?
            .to_string();

        let owner = value
            .pointer("/ownership/owner")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("owner").and_then(serde_json::Value::as_str))
            .ok_or_else(|| BlockchainError::InvalidAsset("asset owner is missing".to_string()))?
            .to_string();

        let collection = value
            .pointer("/grouping/0/group_value")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("collection").and_then(serde_json::Value::as_str))
            .unwrap_or_default()
            .to_string();

        let asset_type = value
            .pointer("/content/metadata/attributes")
            .and_then(serde_json::Value::as_array)
            .and_then(|attributes| {
                attributes.iter().find_map(|attribute| {
                    if attribute.get("trait_type")?.as_str()? == "asset_type" {
                        attribute.get("value")?.as_str()
                    } else {
                        None
                    }
                })
            })
            .or_else(|| value.get("asset_type").and_then(serde_json::Value::as_str))
            .unwrap_or("unknown")
            .to_string();

        let asset_kind = match value
            .pointer("/content/metadata/attributes")
            .and_then(serde_json::Value::as_array)
            .and_then(|attributes| {
                attributes.iter().find_map(|attribute| {
                    let trait_type = attribute.get("trait_type")?.as_str()?;
                    match trait_type {
                        "asset_kind" | "asset_type" => attribute.get("value")?.as_str(),
                        _ => None,
                    }
                })
            })
            .or_else(|| value.get("asset_kind").and_then(serde_json::Value::as_str))
            .or_else(|| value.get("asset_type").and_then(serde_json::Value::as_str))
        {
            Some("land") => AssetKind::Land,
            Some("building") => AssetKind::Building,
            Some(kind) if collection.to_ascii_lowercase().contains("land") => {
                if kind.eq_ignore_ascii_case("building") {
                    AssetKind::Building
                } else {
                    AssetKind::Land
                }
            },
            Some(kind) if collection.to_ascii_lowercase().contains("building") => {
                if kind.eq_ignore_ascii_case("land") {
                    AssetKind::Land
                } else {
                    AssetKind::Building
                }
            },
            _ => {
                return Err(BlockchainError::InvalidAsset(format!(
                    "unsupported asset type {asset_type}"
                )));
            },
        };

        let game_asset_id = value
            .pointer("/content/metadata/attributes")
            .and_then(serde_json::Value::as_array)
            .and_then(|attributes| {
                attributes.iter().find_map(|attribute| {
                    if attribute.get("trait_type")?.as_str()? == "game_asset_id" {
                        attribute.get("value")?.as_str()
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                value
                    .get("game_asset_id")
                    .and_then(serde_json::Value::as_str)
            })
            .map(str::to_string);

        let asset_id = value
            .pointer("/content/metadata/name")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("asset_id").and_then(serde_json::Value::as_str))
            .unwrap_or(&token_id)
            .to_string();

        Ok(Some(NftAsset {
            token_id,
            collection,
            owner,
            metadata: NftMetadata {
                asset_kind,
                asset_id,
                asset_type: asset_type.clone(),
                asset_version: 1,
                game_asset_id,
                allowed_buildings: Vec::new(),
            },
        }))
    }
}

#[async_trait]
impl BlockchainProvider for SolanaBlockchainProvider {
    async fn get_asset(&self, token_id: &str) -> Result<Option<NftAsset>, BlockchainError> {
        let result = self.call("getAsset", serde_json::json!([token_id])).await?;
        Self::parse_asset(&result)
    }

    async fn get_assets_by_owner(
        &self,
        wallet: &str,
        collection: &str,
    ) -> Result<Vec<NftAsset>, BlockchainError> {
        let result = self
            .call(
                "getAssetsByOwner",
                serde_json::json!([{
                    "ownerAddress": wallet,
                    "page": 1,
                    "limit": 1000,
                    "options": {
                        "showCollectionMetadata": false
                    }
                }]),
            )
            .await?;

        let items = result
            .get("items")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                BlockchainError::Unavailable("owner response items are missing".to_string())
            })?;

        items
            .iter()
            .map(Self::parse_asset)
            .collect::<Result<Vec<_>, _>>()
            .map(|assets| {
                assets
                    .into_iter()
                    .flatten()
                    .filter(|asset| asset.collection == collection)
                    .collect()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn land_asset() -> NftAsset {
        NftAsset {
            token_id: "land-token".to_string(),
            collection: "lands".to_string(),
            owner: "wallet-a".to_string(),
            metadata: NftMetadata {
                asset_kind: AssetKind::Land,
                asset_id: "TOWN-0001".to_string(),
                asset_type: "town".to_string(),
                asset_version: 1,
                game_asset_id: None,
                allowed_buildings: vec!["blacksmith".to_string()],
            },
        }
    }

    fn building_asset(owner: &str) -> NftAsset {
        NftAsset {
            token_id: "building-token".to_string(),
            collection: "buildings".to_string(),
            owner: owner.to_string(),
            metadata: NftMetadata {
                asset_kind: AssetKind::Building,
                asset_id: "BLACKSMITH-0001".to_string(),
                asset_type: "blacksmith".to_string(),
                asset_version: 1,
                game_asset_id: Some("building.blacksmith.large.v1".to_string()),
                allowed_buildings: Vec::new(),
            },
        }
    }

    #[tokio::test]
    async fn places_building_only_when_wallet_owns_both_assets() {
        let provider = Arc::new(MockBlockchainProvider::default());
        provider.insert(land_asset());
        provider.insert(building_asset("wallet-a"));

        let mut service = PropertyService::new(provider, BlockchainConfig {
            land_collection: "lands".to_string(),
            building_collection: "buildings".to_string(),
            ..Default::default()
        });

        service.register_parcel(LandParcel {
            land_id: "TOWN-0001".to_string(),
            token_id: "land-token".to_string(),
            world_id: "WORLD-001".to_string(),
            origin: (0, 0),
            size: (500, 500),
            land_type: "town".to_string(),
            allowed_buildings: vec!["blacksmith".to_string()],
            max_buildings: 1,
        });

        let placement = service
            .place_building(
                "wallet-a",
                "TOWN-0001",
                "building-token",
                (10, 20, 30),
                90.0,
            )
            .await
            .expect("authorized placement should succeed");

        assert_eq!(placement.position, (10, 20, 30));
    }

    #[tokio::test]
    async fn authorizes_building_using_linked_player_wallet() {
        let provider = Arc::new(MockBlockchainProvider::default());
        provider.insert(land_asset());
        provider.insert(building_asset("wallet-a"));

        let mut service = PropertyService::new(provider, BlockchainConfig {
            land_collection: "lands".to_string(),
            building_collection: "buildings".to_string(),
            ..Default::default()
        });
        service.register_parcel(LandParcel {
            land_id: "TOWN-0001".to_string(),
            token_id: "land-token".to_string(),
            world_id: "WORLD-001".to_string(),
            origin: (0, 0),
            size: (500, 500),
            land_type: "town".to_string(),
            allowed_buildings: vec!["blacksmith".to_string()],
            max_buildings: 1,
        });

        let mut wallet_registry = PlayerWalletRegistry::default();
        wallet_registry.associate_wallet("player-42", "wallet-a");

        let placement = service
            .authorize_placement_for_player(
                "player-42",
                &wallet_registry,
                "TOWN-0001",
                "building-token",
                (10, 20, 30),
                90.0,
            )
            .await
            .expect("linked player wallet should be authorized");

        assert_eq!(placement.position, (10, 20, 30));
    }

    #[tokio::test]
    async fn rejects_building_when_wallet_does_not_own_land() {
        let provider = Arc::new(MockBlockchainProvider::default());
        provider.insert(land_asset());
        provider.insert(building_asset("wallet-a"));

        let mut service = PropertyService::new(provider, BlockchainConfig {
            land_collection: "lands".to_string(),
            building_collection: "buildings".to_string(),
            ..Default::default()
        });

        service.register_parcel(LandParcel {
            land_id: "TOWN-0001".to_string(),
            token_id: "land-token".to_string(),
            world_id: "WORLD-001".to_string(),
            origin: (0, 0),
            size: (500, 500),
            land_type: "town".to_string(),
            allowed_buildings: vec!["blacksmith".to_string()],
            max_buildings: 1,
        });

        let error = service
            .place_building("wallet-b", "TOWN-0001", "building-token", (10, 20, 30), 0.0)
            .await
            .expect_err("foreign wallet must be rejected");

        assert_eq!(
            error,
            PropertyError::Blockchain(BlockchainError::InvalidAsset(
                "ownership, collection, or asset kind verification failed".to_string()
            ))
        );
    }

    #[test]
    fn associates_wallet_only_with_valid_one_time_signature() {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let challenge = "server-challenge";
        let message = wallet_association_message("player-42", challenge);
        let signature = bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string();
        let mut registry = PlayerWalletRegistry::default();
        registry.issue_challenge("player-42", challenge);

        registry
            .associate_wallet_signed("player-42", &wallet, &signature)
            .expect("valid wallet signature should be accepted");
        assert_eq!(
            registry.wallet_for_player("player-42"),
            Some(wallet.as_str())
        );

        assert_eq!(
            registry.associate_wallet_signed("player-42", &wallet, &signature),
            Err(PropertyError::WalletChallengeMissing)
        );
    }

    #[test]
    fn rejects_invalid_wallet_signature_without_consuming_challenge() {
        let signing_key = SigningKey::from_bytes(&[8; 32]);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let mut registry = PlayerWalletRegistry::default();
        registry.issue_challenge("player-42", "server-challenge");

        assert_eq!(
            registry.associate_wallet_signed("player-42", &wallet, "invalid-signature"),
            Err(PropertyError::InvalidWalletSignature)
        );
        assert_eq!(registry.wallet_for_player("player-42"), None);
    }
}
