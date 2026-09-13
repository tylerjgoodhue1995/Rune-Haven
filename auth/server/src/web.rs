use crate::auth::{self, db, AuthError};
use crate::ratelimit::RateLimiter;
use auth_common::{
    ChangePasswordPayload, ChangeUsernamePayload, DeleteAccountPayload, DeleteAccountResponse,
    RegisterPayload, SignInPayload, SignInResponse, UsernameLookupPayload, UsernameLookupResponse,
    UuidLookupPayload, UuidLookupResponse, ValidityCheckPayload, ValidityCheckResponse,
};
use lazy_static::lazy_static;
use log::*;
use rouille::{start_server, Request, Response};
use std::net::IpAddr;

lazy_static! {
    static ref RATELIMITER: RateLimiter = RateLimiter::new();
}

fn legal_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || ['-', '_'].contains(&c)
}

fn verify_username(username: &str) -> Result<(), AuthError> {
    if !(3..=32).contains(&username.len()) {
        Err(AuthError::InvalidRequest(
            "Username must be between 3 and 32 characters inclusive.".into(),
        ))
    } else if !username.chars().all(legal_char) {
        Err(AuthError::InvalidRequest(
            "Illegal character in username.".into(),
        ))
    } else {
        Ok(())
    }
}

/// Limit all request bodies to max 5 KiB.
///
/// The current largest request, `register`, is at most a 32 character long
/// username and a 64 byte long password hash. So currently this limit is
/// generous.
const MAX_REQUEST_SIZE: u64 = 1024 * 5;

fn take_request_data(req: &Request) -> Result<Vec<u8>, AuthError> {
    use std::io::Read;

    let body = req.data().ok_or_else(|| {
        log::error!("Called `Request::data() more than once`");

        AuthError::InternalServerError
    })?;

    let mut buf = Vec::new();

    body.take(MAX_REQUEST_SIZE + 1)
        .read_to_end(&mut buf)
        .map_err(|err| {
            log::error!("Failed to read request: {err}");

            AuthError::Io(err)
        })?;

    if buf.len() as u64 > MAX_REQUEST_SIZE {
        return Err(AuthError::RequestTooLarge);
    }

    Ok(buf)
}

fn sizelimit(
    req: &Request,
    f: fn(&[u8]) -> Result<Response, AuthError>,
) -> Result<Response, AuthError> {
    f(&take_request_data(req)?)
}

fn ratelimit(
    req: &Request,
    f: fn(&[u8]) -> Result<Response, AuthError>,
) -> Result<Response, AuthError> {
    if RATELIMITER.check(remote(req)) {
        sizelimit(req, f)
    } else {
        Err(AuthError::RateLimit)
    }
}

fn remote(req: &Request) -> IpAddr {
    req.header("X-Real-IP")
        .and_then(|ip| ip.parse().ok())
        .unwrap_or(req.remote_addr().ip())
}

fn ping(req: &Request) -> Response {
    Response::text(format!("Ping! {}", remote(req)))
}

fn username_to_uuid(body: &[u8]) -> Result<Response, AuthError> {
    let payload: UuidLookupPayload = serde_json::from_slice(body)?;
    let db = db()?;
    let uuid = auth::username_to_uuid(&payload.username, &db)?;
    let response = UuidLookupResponse { uuid };
    Ok(Response::json(&response))
}

fn uuid_to_username(body: &[u8]) -> Result<Response, AuthError> {
    let payload: UsernameLookupPayload = serde_json::from_slice(body)?;
    let db = db()?;
    let username = auth::uuid_to_username(&payload.uuid, &db)?;
    let response = UsernameLookupResponse { username };
    Ok(Response::json(&response))
}

fn register(body: &[u8]) -> Result<Response, AuthError> {
    let payload: RegisterPayload = serde_json::from_slice(body)?;

    // We expect argon hashes to be 64 characters long, any longer is a garbage request.
    if payload.password.len() > 64 {
        return Err(AuthError::RequestTooLarge);
    }

    verify_username(&payload.username)?;
    let db = db()?;
    auth::register(&payload.username, &payload.password, &db)?;
    Ok(Response::text("Ok"))
}

fn generate_token(body: &[u8]) -> Result<Response, AuthError> {
    let payload: SignInPayload = serde_json::from_slice(body)?;
    verify_username(&payload.username)?;
    let db = db()?;
    let token = auth::generate_token(&payload.username, &payload.password, &db)?;
    let response = SignInResponse { token };
    Ok(Response::json(&response))
}

fn delete_account(body: &[u8]) -> Result<Response, AuthError> {
    let payload: DeleteAccountPayload = serde_json::from_slice(body)?;
    verify_username(&payload.username)?;
    let mut db = db()?;
    auth::delete_account(&payload.username, &payload.password, &mut db)?;
    let response = DeleteAccountResponse {};
    Ok(Response::json(&response))
}

fn verify(body: &[u8]) -> Result<Response, AuthError> {
    let payload: ValidityCheckPayload = serde_json::from_slice(body)?;
    let uuid = auth::verify(payload.token)?;
    let response = ValidityCheckResponse { uuid };
    Ok(Response::json(&response))
}

fn change_password(body: &[u8]) -> Result<Response, AuthError> {
    let payload: ChangePasswordPayload = serde_json::from_slice(body)?;
    verify_username(&payload.username)?;
    let db = db()?;
    auth::change_password(payload, &db)?;
    Ok(Response::text("Ok"))
}

fn change_username(body: &[u8]) -> Result<Response, AuthError> {
    let payload: ChangeUsernamePayload = serde_json::from_slice(body)?;
    verify_username(&payload.old_username)?;
    verify_username(&payload.new_username)?;
    let mut db = db()?;
    auth::change_username(payload, &mut db)?;
    Ok(Response::text("Ok"))
}

pub fn start() {
    let addr = "0.0.0.0:19253";
    debug!("Starting webserver on {}", addr);

    start_server(addr, move |request| {
        debug!("[{}] -> {}", remote(request), request.url());

        let path = request.raw_url().split('?').next().unwrap();

        let response = match (request.method(), path) {
            ("GET", "/ping") => ping(request),
            ("POST", path) => {
                let result = match path {
                    "/username_to_uuid" => sizelimit(request, username_to_uuid),
                    "/uuid_to_username" => sizelimit(request, uuid_to_username),
                    "/register" => ratelimit(request, register),
                    "/generate_token" => ratelimit(request, generate_token),
                    "/delete_account" => ratelimit(request, delete_account),
                    "/verify" => sizelimit(request, verify),
                    "/change_password" => ratelimit(request, change_password),
                    "/change_username" => ratelimit(request, change_username),
                    _ => Ok(Response::empty_404()),
                };

                match result {
                    Ok(response) => response,
                    Err(err) => {
                        info!("[{}:{}] rejected: {}", remote(request), path, err);

                        Response::text(format!("{}", err)).with_status_code(err.status_code())
                    }
                }
            }
            _ => Response::empty_404(),
        };

        response.with_unique_header("Access-Control-Allow-Origin", "*")
    });
}
