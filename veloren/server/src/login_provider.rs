use crate::{
    Client,
    property::wallet_link_message,
    settings::{AdminRecord, Ban, Banlist, WhitelistRecord, banlist::NormalizedIpAddr},
};
use authc::{AuthClient, AuthClientError, AuthToken, Uuid};
use bs58;
use chrono::Utc;
use common::comp::AdminRole;
use common_net::msg::RegisterError;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hashbrown::HashMap;
use specs::Component;
use std::{fs, path::PathBuf, str::FromStr, sync::Arc};
use tokio::{runtime::Runtime, sync::oneshot};
use tracing::{error, info};

fn parse_wallet_challenge(challenge: &str) -> Result<u128, RegisterError> {
    let trimmed = challenge.trim();
    if trimmed.is_empty() {
        return Err(RegisterError::AuthError(
            "Invalid wallet challenge".to_string(),
        ));
    }

    if let Ok(value) = trimmed.parse::<u128>() {
        return Ok(value);
    }

    let challenge_value = trimmed
        .rsplit(':')
        .next()
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u128>().ok())
        .ok_or_else(|| RegisterError::AuthError("Invalid wallet challenge".to_string()))?;

    Ok(challenge_value)
}

/// Determines whether a user is banned, given a ban record connected to a user,
/// the `AdminRecord` of that user (if it exists), and the current time.
pub fn ban_applies(
    ban: &Ban,
    admin: Option<&AdminRecord>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    // Make sure the ban is active, and that we can't override it.
    //
    // If we are an admin and our role is at least as high as the role of the
    // person who banned us, we can override the ban; we negate this to find
    // people who cannot override it.
    let exceeds_ban_role = |admin: &AdminRecord| {
        AdminRole::from(admin.role) >= AdminRole::from(ban.performed_by_role())
    };
    !ban.is_expired(now) && !admin.is_some_and(exceeds_ban_role)
}

fn derive_uuid(username: &str) -> Uuid { Uuid::from_u128(derive_uuid_value(username)) }

fn derive_uuid_value(username: &str) -> u128 {
    let mut state = 144066263297769815596495629667062367629;

    for byte in username.as_bytes() {
        state ^= *byte as u128;
        state = state.wrapping_mul(309485009821345068724781371);
    }

    state
}

// Admin wallet address
const ADMIN_WALLET: &str = "EiL5hGfzLAyCah2GMrxFz47HPLwgK6CQtS1CL1gWxQF8";

pub fn is_admin_wallet(wallet: &str) -> bool {
    wallet == ADMIN_WALLET
}

fn wallet_username(wallet: &str) -> String {
    format!("w{:031x}", derive_uuid_value(wallet) & (u128::MAX >> 4))
}

/// derive Uuid for "singleplayer" is a pub fn
pub fn derive_singleplayer_uuid() -> Uuid { derive_uuid("singleplayer") }

pub struct PendingLogin {
    pending_r: oneshot::Receiver<Result<(String, Uuid), RegisterError>>,
}

impl PendingLogin {
    pub(crate) fn new_success(username: String, uuid: Uuid) -> Self {
        let (pending_s, pending_r) = oneshot::channel();
        let _ = pending_s.send(Ok((username, uuid)));

        Self { pending_r }
    }

    pub(crate) fn new_error(error: RegisterError) -> Self {
        let (pending_s, pending_r) = oneshot::channel();
        let _ = pending_s.send(Err(error));
        Self { pending_r }
    }
}

impl Component for PendingLogin {
    type Storage = specs::DenseVecStorage<Self>;
}

pub struct LoginProvider {
    runtime: Arc<Runtime>,
    auth_server: Option<Arc<AuthClient>>,
    alpha_access_path: Option<PathBuf>,
}

#[derive(serde::Deserialize)]
struct AlphaAccessState {
    maintenance_mode: bool,
    allowed_wallets: Vec<String>,
}

impl LoginProvider {
    pub fn new(auth_addr: Option<String>, runtime: Arc<Runtime>) -> Self {
        tracing::trace!(?auth_addr, "Starting LoginProvider");

        let auth_server = auth_addr.map(|addr| {
            let (scheme, authority) = addr.split_once("://").expect("invalid auth url");

            let scheme = scheme
                .parse::<authc::Scheme>()
                .expect("invalid auth url scheme");
            let authority = authority
                .parse::<authc::Authority>()
                .expect("invalid auth url authority");

            Arc::new(AuthClient::new(scheme, authority).expect("insecure auth scheme"))
        });

        Self {
            runtime,
            auth_server,
            alpha_access_path: std::env::var_os("VELOREN_ALPHA_ACCESS_PATH").map(PathBuf::from),
        }
    }

    pub fn verify(&self, username_or_token: &str) -> PendingLogin {
        let (pending_s, pending_r) = oneshot::channel();

        match &self.auth_server {
            // Token from auth server expected
            Some(srv) => {
                let srv = Arc::clone(srv);
                let username_or_token = username_or_token.to_string();
                self.runtime.spawn(async move {
                    let _ = pending_s.send(Self::query(srv, &username_or_token).await);
                });
            },
            // Username is expected
            None => {
                let username = username_or_token;
                let uuid = derive_uuid(username);
                let _ = pending_s.send(Ok((username.to_string(), uuid)));
            },
        }

        PendingLogin { pending_r }
    }

    pub fn verify_wallet(
        wallet: &str,
        challenge: &str,
        signature: &str,
    ) -> Result<(String, Uuid), RegisterError> {
        let challenge_nanos = parse_wallet_challenge(challenge)?;
        let now_nanos = Utc::now()
            .timestamp_nanos_opt()
            .ok_or_else(|| RegisterError::AuthError("Invalid server clock".to_string()))?
            as u128;
        if now_nanos.abs_diff(challenge_nanos) > 5 * 60 * 1_000_000_000 {
            return Err(RegisterError::AuthError(
                "Wallet challenge expired".to_string(),
            ));
        }

        let public_key = bs58::decode(wallet)
            .into_vec()
            .map_err(|_| RegisterError::AuthError("Invalid wallet address".to_string()))?;
        let public_key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| RegisterError::AuthError("Invalid wallet address".to_string()))?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| RegisterError::AuthError("Invalid wallet address".to_string()))?;
        let signature = bs58::decode(signature)
            .into_vec()
            .map_err(|_| RegisterError::AuthError("Invalid wallet signature".to_string()))?;
        let signature = Signature::from_slice(&signature)
            .map_err(|_| RegisterError::AuthError("Invalid wallet signature".to_string()))?;
        verifying_key
            .verify(
                wallet_link_message(wallet, challenge).as_bytes(),
                &signature,
            )
            .map_err(|_| RegisterError::AuthError("Wallet signature rejected".to_string()))?;

        Ok((wallet_username(wallet), derive_uuid(wallet)))
    }

    fn check_alpha_access(&self, uuid: Uuid) -> Result<(), RegisterError> {
        let Some(path) = &self.alpha_access_path else {
            return Ok(());
        };
        let bytes = fs::read(path).map_err(|error| {
            RegisterError::AuthError(format!("Alpha access configuration unavailable: {error}"))
        })?;
        let access: AlphaAccessState = serde_json::from_slice(&bytes).map_err(|error| {
            RegisterError::AuthError(format!("Invalid alpha access configuration: {error}"))
        })?;

        if !access.maintenance_mode
            || uuid == derive_uuid(ADMIN_WALLET)
            || access
                .allowed_wallets
                .iter()
                .any(|wallet| derive_uuid(wallet) == uuid)
        {
            Ok(())
        } else {
            Err(RegisterError::NotOnWhitelist)
        }
    }

    pub(crate) fn login<R>(
        &self,
        pending: &mut PendingLogin,
        client: &Client,
        admins: &HashMap<Uuid, AdminRecord>,
        whitelist: &HashMap<Uuid, WhitelistRecord>,
        banlist: &Banlist,
        player_count_exceeded: impl FnOnce(String, Uuid) -> (bool, R),
        make_ip_ban_upgrade: impl FnOnce(NormalizedIpAddr, Uuid, String),
    ) -> Option<Result<R, RegisterError>> {
        match pending.pending_r.try_recv() {
            Ok(Err(e)) => Some(Err(e)),
            Ok(Ok((username, uuid))) => {
                let now = Utc::now();
                // We ignore mpsc connections since those aren't to an external
                // process.
                let ip = client
                    .connected_from_addr()
                    .socket_addr()
                    .map(|s| s.ip())
                    .map(NormalizedIpAddr::from);
                // Hardcoded admins can always log in.
                let admin = admins.get(&uuid);
                if admin.is_none()
                    && let Err(error) = self.check_alpha_access(uuid)
                {
                    return Some(Err(error));
                }
                if let Some(ban) = banlist
                    .uuid_bans()
                    .get(&uuid)
                    .and_then(|ban_entry| ban_entry.current.action.ban())
                    .into_iter()
                    .chain(ip.and_then(|ip| {
                        banlist
                            .ip_bans()
                            .get(&ip)
                            .and_then(|ban_entry| ban_entry.current.action.ban())
                    }))
                    .find(|ban| ban_applies(ban, admin, now))
                {
                    if let Some(ip) = ip
                        && ban.upgrade_to_ip
                    {
                        make_ip_ban_upgrade(ip, uuid, username.clone());
                    }

                    // Get ban info and send a copy of it
                    return Some(Err(RegisterError::Banned(ban.info())));
                }

                // non-admins can only join if the whitelist is empty (everyone can join)
                // or their name is in the whitelist.
                if admin.is_none() && !whitelist.is_empty() && !whitelist.contains_key(&uuid) {
                    return Some(Err(RegisterError::NotOnWhitelist));
                }

                // non-admins can only join if the player count has not been exceeded.
                let (player_count_exceeded, res) = player_count_exceeded(username, uuid);
                if admin.is_none() && player_count_exceeded {
                    return Some(Err(RegisterError::TooManyPlayers));
                }

                Some(Ok(res))
            },
            Err(oneshot::error::TryRecvError::Closed) => {
                error!("channel got closed to early, this shouldn't happen");
                Some(Err(RegisterError::AuthError(
                    "Internal Error verifying".to_string(),
                )))
            },
            Err(oneshot::error::TryRecvError::Empty) => None,
        }
    }

    async fn query(
        srv: Arc<AuthClient>,
        username_or_token: &str,
    ) -> Result<(String, Uuid), RegisterError> {
        info!(?username_or_token, "Validating token");
        // Parse token
        let token = AuthToken::from_str(username_or_token)
            .map_err(|e| RegisterError::AuthError(e.to_string()))?;
        // Validate token
        match async {
            let uuid = srv.validate(token).await?;
            let username = srv.uuid_to_username(uuid).await?;
            let r: Result<_, AuthClientError> = Ok((username, uuid));
            r
        }
        .await
        {
            Err(e) => Err(RegisterError::AuthError(e.to_string())),
            Ok((username, uuid)) => Ok((username, uuid)),
        }
    }

    pub fn username_to_uuid(&self, username: &str) -> Result<Uuid, AuthClientError> {
        match &self.auth_server {
            Some(srv) => {
                //TODO: optimize
                self.runtime.block_on(srv.username_to_uuid(&username))
            },
            None => Ok(derive_uuid(username)),
        }
    }

    pub fn uuid_to_username(
        &self,
        uuid: Uuid,
        fallback_alias: &str,
    ) -> Result<String, AuthClientError> {
        match &self.auth_server {
            Some(srv) => {
                //TODO: optimize
                self.runtime.block_on(srv.uuid_to_username(uuid))
            },
            None => Ok(fallback_alias.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LoginProvider, wallet_username};
    use crate::property::wallet_link_message;
    use common::comp::Player;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn wallet_username_is_a_valid_alias() {
        let username = wallet_username("7xKXtg2kJ5G6vQw8rY9uP3mN4bC1dE2fH6jL8sT0vW");

        assert_eq!(username.len(), common::comp::MAX_ALIAS_LEN);
        assert!(Player::alias_validate(&username).is_ok());
    }

    #[test]
    fn wallet_login_accepts_server_style_challenges() {
        let signing_key = SigningKey::from_bytes(&[123u8; 32]);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let challenge = format!(
            "challenge:player-1:{}",
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("valid clock")
        );
        let signature = bs58::encode(
            signing_key
                .sign(wallet_link_message(&wallet, &challenge).as_bytes())
                .to_bytes(),
        )
        .into_string();

        let result = LoginProvider::verify_wallet(&wallet, &challenge, &signature);
        assert!(
            result.is_ok(),
            "expected wallet login to accept challenge string format"
        );
    }
}
