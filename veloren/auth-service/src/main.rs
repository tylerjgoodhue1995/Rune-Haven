use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::random;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthToken {
    pub unique: u64,
}

impl AuthToken {
    pub fn generate() -> Self { Self { unique: random() } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterPayload {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInPayload {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SignInResponse {
    pub token: AuthToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidityCheckPayload {
    pub token: AuthToken,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ValidityCheckResponse {
    pub uuid: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UuidLookupPayload {
    pub username: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UuidLookupResponse {
    pub uuid: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsernameLookupPayload {
    pub uuid: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsernameLookupResponse {
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRecord {
    uuid: Uuid,
    expires_at_unix_ms: u128,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistentState {
    users: HashMap<String, String>,
    username_to_uuid: HashMap<String, Uuid>,
    uuid_to_username: HashMap<Uuid, String>,
    #[serde(default)]
    tokens: HashMap<String, TokenRecord>,
    admins: HashSet<String>,
}

#[derive(Clone)]
struct AppState {
    users: Arc<RwLock<HashMap<String, String>>>,
    username_to_uuid: Arc<RwLock<HashMap<String, Uuid>>>,
    uuid_to_username: Arc<RwLock<HashMap<Uuid, String>>>,
    tokens: Arc<RwLock<HashMap<AuthToken, TokenRecord>>>,
    admins: Arc<RwLock<HashSet<String>>>,
    rate_limiter: Arc<RateLimiter>,
    data_path: PathBuf,
    token_ttl: Duration,
}

#[derive(Clone)]
struct RateLimiter {
    max_requests: usize,
    window: Duration,
    requests: Arc<RwLock<VecDeque<Instant>>>,
}

impl RateLimiter {
    fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            requests: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    fn try_acquire(&self) -> bool {
        let now = Instant::now();
        let mut requests = self.requests.write().unwrap();
        while requests
            .front()
            .is_some_and(|ts| now.duration_since(*ts) > self.window)
        {
            requests.pop_front();
        }
        if requests.len() >= self.max_requests {
            return false;
        }
        requests.push_back(now);
        true
    }
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: message.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response { (self.status, self.message).into_response() }
}

fn legal_char(c: char) -> bool { c.is_ascii_alphanumeric() || matches!(c, '-' | '_') }

fn validate_username(username: &str) -> Result<(), AppError> {
    if !(3..=32).contains(&username.len()) {
        return Err(AppError::bad_request(
            "Username must be between 3 and 32 characters inclusive.",
        ));
    }
    if !username.chars().all(legal_char) {
        return Err(AppError::bad_request("Illegal character in username."));
    }
    Ok(())
}

fn username_to_uuid(username: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, username.as_bytes())
}

fn net_prehash(password: &str) -> String {
    let salt = fxhash::hash64(password).to_le_bytes();
    let mut hasher = Sha256::new();
    hasher.update(&salt);
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

fn file_now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis()
}

fn persist_state(state: &AppState) {
    let persistent = PersistentState {
        users: state.users.read().unwrap().clone(),
        username_to_uuid: state.username_to_uuid.read().unwrap().clone(),
        uuid_to_username: state.uuid_to_username.read().unwrap().clone(),
        tokens: state
            .tokens
            .read()
            .unwrap()
            .iter()
            .map(|(token, record)| (token.unique.to_string(), record.clone()))
            .collect(),
        admins: state.admins.read().unwrap().clone(),
    };

    if let Some(parent) = state.data_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let bytes =
        serde_json::to_vec_pretty(&persistent).expect("persisted auth state must serialize");
    let _ = fs::write(&state.data_path, bytes);
}

fn load_persistent_state(data_path: &Path) -> PersistentState {
    match fs::read(data_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => PersistentState::default(),
    }
}

fn sanitized_admins() -> HashSet<String> {
    std::env::var("BETA_AUTH_ADMINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

impl AppState {
    fn new(data_path: PathBuf, token_ttl: Duration) -> Self {
        let persistent = load_persistent_state(&data_path);
        let PersistentState {
            users,
            username_to_uuid,
            uuid_to_username,
            tokens: persisted_tokens,
            admins,
        } = persistent;
        let tokens = persisted_tokens
            .into_iter()
            .filter_map(|(key, record)| {
                let unique = key.parse::<u64>().ok()?;
                Some((AuthToken { unique }, record))
            })
            .collect();

        let state = Self {
            users: Arc::new(RwLock::new(users)),
            username_to_uuid: Arc::new(RwLock::new(username_to_uuid)),
            uuid_to_username: Arc::new(RwLock::new(uuid_to_username)),
            tokens: Arc::new(RwLock::new(tokens)),
            admins: Arc::new(RwLock::new(admins)),
            rate_limiter: Arc::new(RateLimiter::new(
                std::env::var("BETA_AUTH_RATE_LIMIT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(120),
                Duration::from_secs(
                    std::env::var("BETA_AUTH_RATE_WINDOW_SECS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(60),
                ),
            )),
            data_path,
            token_ttl,
        };

        let admins = sanitized_admins();
        *state.admins.write().unwrap() = admins;
        state
    }

    fn ensure_rate_limit(&self) -> Result<(), AppError> {
        if !self.rate_limiter.try_acquire() {
            return Err(AppError::too_many_requests(
                "Too many requests. Please slow down.",
            ));
        }
        Ok(())
    }

    fn prune_expired_tokens(&self) {
        let now = file_now_ms();
        let mut tokens = self.tokens.write().unwrap();
        tokens.retain(|_, record| record.expires_at_unix_ms > now);
    }
}

async fn ping() -> &'static str { "Ping!" }

fn parse_json<T: for<'de> Deserialize<'de>>(body: Bytes) -> Result<T, AppError> {
    serde_json::from_slice(&body).map_err(|error| AppError::bad_request(error.to_string()))
}

async fn register_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let payload: RegisterPayload = parse_json(body)?;
    state.ensure_rate_limit()?;
    validate_username(&payload.username)?;

    let uuid = username_to_uuid(&payload.username);
    {
        let mut users = state.users.write().unwrap();
        if users.contains_key(&payload.username) {
            return Err(AppError::bad_request("Username is already taken."));
        }
        users.insert(payload.username.clone(), net_prehash(&payload.password));
    }

    {
        let mut username_map = state.username_to_uuid.write().unwrap();
        username_map.insert(payload.username.clone(), uuid);
    }

    {
        let mut uuid_map = state.uuid_to_username.write().unwrap();
        uuid_map.insert(uuid, payload.username.clone());
    }

    persist_state(&state);
    Ok(StatusCode::OK)
}

async fn username_to_uuid_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<UuidLookupResponse>, AppError> {
    let payload: UuidLookupPayload = parse_json(body)?;
    state.ensure_rate_limit()?;
    let uuid = state
        .username_to_uuid
        .read()
        .unwrap()
        .get(&payload.username)
        .copied()
        .ok_or_else(|| AppError::bad_request("Unknown username."))?;

    Ok(Json(UuidLookupResponse { uuid }))
}

async fn uuid_to_username_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<UsernameLookupResponse>, AppError> {
    let payload: UsernameLookupPayload = parse_json(body)?;
    state.ensure_rate_limit()?;
    let username = state
        .uuid_to_username
        .read()
        .unwrap()
        .get(&payload.uuid)
        .cloned()
        .ok_or_else(|| AppError::bad_request("Unknown uuid."))?;

    Ok(Json(UsernameLookupResponse { username }))
}

async fn generate_token_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<SignInResponse>, AppError> {
    let payload: SignInPayload = parse_json(body)?;
    state.ensure_rate_limit()?;
    validate_username(&payload.username)?;

    state.prune_expired_tokens();

    let stored = {
        let users = state.users.read().unwrap();
        users
            .get(&payload.username)
            .cloned()
            .ok_or_else(|| AppError::unauthorized("Unknown username."))?
    };

    if stored != net_prehash(&payload.password) {
        return Err(AppError::unauthorized("Invalid password."));
    }

    let uuid = {
        let username_to_uuid = state.username_to_uuid.read().unwrap();
        username_to_uuid
            .get(&payload.username)
            .copied()
            .expect("username_to_uuid should always exist")
    };

    let token = AuthToken::generate();
    {
        let mut tokens = state.tokens.write().unwrap();
        tokens.insert(token, TokenRecord {
            uuid,
            expires_at_unix_ms: file_now_ms() + state.token_ttl.as_millis(),
        });
    }
    persist_state(&state);
    Ok(Json(SignInResponse { token }))
}

async fn verify_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<ValidityCheckResponse>, AppError> {
    let payload: ValidityCheckPayload = parse_json(body)?;
    state.ensure_rate_limit()?;
    state.prune_expired_tokens();

    let record = {
        let mut tokens = state.tokens.write().unwrap();
        let record = tokens
            .get(&payload.token)
            .cloned()
            .ok_or_else(|| AppError::unauthorized("Invalid token."))?;

        if record.expires_at_unix_ms <= file_now_ms() {
            tokens.remove(&payload.token);
            drop(tokens);
            persist_state(&state);
            return Err(AppError::unauthorized("Token expired."));
        }

        record
    };

    Ok(Json(ValidityCheckResponse { uuid: record.uuid }))
}

#[tokio::main]
async fn main() {
    let bind = std::env::var("BETA_AUTH_BIND").unwrap_or_else(|_| "0.0.0.0:19253".to_string());
    let data_path = std::env::var("BETA_AUTH_DATA_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("auth-data/auth-state.json"));
    let token_ttl = Duration::from_secs(
        std::env::var("BETA_AUTH_TOKEN_TTL_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(7 * 24 * 60 * 60),
    );

    let state = AppState::new(data_path.clone(), token_ttl);

    let app = Router::new()
        .route("/ping", get(ping))
        .route("/register", post(register_handler))
        .route("/generate_token", post(generate_token_handler))
        .route("/verify", post(verify_handler))
        .route("/username_to_uuid", post(username_to_uuid_handler))
        .route("/uuid_to_username", post(uuid_to_username_handler))
        .with_state(state);

    let addr: SocketAddr = bind
        .parse()
        .expect("BETA_AUTH_BIND must be a valid socket address");

    println!("Starting beta auth server on {}", addr);
    println!("Auth state persisted at {}", data_path.display());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind auth service");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server failed");
}
