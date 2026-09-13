#![forbid(unsafe_code)]

use argon2::{Config, Variant};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Pre-hashes a password to be sent over the network.
///
/// This is in `common` because it also is used by `server` in tests.
pub fn net_prehash(password: &str) -> String {
    let salt = fxhash::hash64(password);
    let config = Config {
        variant: Variant::Argon2i,
        time_cost: 3,
        mem_cost: 4096,
        ..Default::default()
    };
    let bytes = argon2::hash_raw(password.as_bytes(), &salt.to_le_bytes(), &config).unwrap();
    hex::encode(bytes)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AuthToken {
    pub unique: u64,
}

impl std::str::FromStr for AuthToken {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse::<u64>() {
            Ok(s) => Ok(AuthToken { unique: s }),
            Err(e) => Err(e),
        }
    }
}

impl AuthToken {
    pub fn generate() -> Self {
        Self {
            unique: rand::random(),
        }
    }

    pub fn serialize(&self) -> String {
        self.unique.to_string()
    }

    pub fn deserialize(s: &str) -> Self {
        let n = s.parse().unwrap();
        Self { unique: n }
    }
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
pub struct DeleteAccountPayload {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAccountResponse {}

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
pub struct ChangePasswordPayload {
    pub username: String,
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeUsernamePayload {
    pub old_username: String,
    pub password: String,
    pub new_username: String,
}
