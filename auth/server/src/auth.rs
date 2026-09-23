use crate::cache::TimedCache;
use argon2::{Error as HashError, Variant};
use auth_common::{AuthToken, ChangePasswordPayload, ChangeUsernamePayload};
use chrono::Utc;
use lazy_static::lazy_static;
use rusqlite::{params, Connection, Error as DbError};
use serde_json::Error as JsonError;
use std::error::Error;
use std::fmt;
use std::{env, path::PathBuf};
use uuid::Uuid;

/// How long usernames are kept after an account is deleted or renamed (in days)
const USER_RESERVED_NAME_KEEP: i64 = 30;
/// How long it takes until an account can change its name after changing
const USER_CHANGE_INTERVAL: i64 = 30;

lazy_static! {
    static ref TOKENS: TimedCache = TimedCache::new();
}

fn apply_db_dir_override(db_dir: &str) -> String {
    if let Some(val) = env::var_os("AUTH_DB_DIR") {
        let path = PathBuf::from(val);
        if path.exists() || path.parent().map(|x| x.exists()).unwrap_or(false) {
            // Only allow paths with valid unicode characters
            if let Some(path) = path.to_str() {
                return path.to_owned();
            }
        }
        log::warn!("AUTH_DB_DIR is an invalid path.");
    }
    db_dir.to_string()
}

/// Get a connection to the database, in tests, an in-memory database is used.
pub(super) fn db() -> Result<Connection, AuthError> {
    if cfg!(test) {
        Ok(Connection::open_in_memory()?)
    } else {
        let default_path = if cfg!(windows) {
            "./data/auth.db"
        } else {
            "/opt/veloren-auth/data/auth.db"
        };
        let db_dir = &apply_db_dir_override(default_path);
        Ok(Connection::open(db_dir)?)
    }
}

fn salt() -> [u8; 16] {
    rand::random::<u128>().to_le_bytes()
}

fn decapitalize(string: &str) -> String {
    string.chars().flat_map(char::to_lowercase).collect()
}

#[derive(Debug)]
pub enum AuthError {
    UserExists,
    UserDoesNotExist,
    UserReserved,
    UserRecentlyChanged,
    InvalidLogin,
    InvalidToken,
    Db(DbError),
    Hash(HashError),
    Json(JsonError),
    InvalidRequest(String),
    RateLimit,
    RequestTooLarge,

    InternalServerError,
    Io(std::io::Error),
}

impl AuthError {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::UserExists => 400,
            Self::UserDoesNotExist => 400,
            Self::UserReserved => 400,
            Self::UserRecentlyChanged => 400,
            Self::InvalidLogin => 400,
            Self::InvalidToken => 400,
            Self::Db(_) => 500,
            Self::Hash(_) => 500,
            Self::Json(_) => 400,
            Self::InvalidRequest(_) => 400,
            Self::RateLimit => 429,
            Self::RequestTooLarge => 413,

            Self::InternalServerError | Self::Io(_) => 500,
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::UserExists => "That username is already taken.".into(),
                Self::UserDoesNotExist => "That user does not exist.".into(),
                Self::UserReserved => "This account was recently deleted or renamed from".into(),
                Self::UserRecentlyChanged => "This account name has recently been changed".into(),
                Self::InvalidLogin =>
                    "The username + password combination was incorrect or the account does not exist."
                        .into(),
                Self::InvalidToken => "The given token is invalid.".into(),
                Self::Db(err) => format!("Database error: {}", err),
                Self::Hash(err) => format!("Error securely storing password: {}", err),
                Self::Json(err) => format!("Error decoding JSON: {}", err),
                Self::InvalidRequest(s) =>
                    format!("The request was invalid in some form. Reason: {}", s),
                Self::RateLimit => "You are sending too many requests. Please slow down.".into(),
                Self::RequestTooLarge => "The request was too large".into(),

                Self::InternalServerError | Self::Io(_) => "Internal server error".into(),
            }
        )
    }
}

impl Error for AuthError {}

impl From<DbError> for AuthError {
    fn from(err: DbError) -> Self {
        Self::Db(err)
    }
}

impl From<HashError> for AuthError {
    fn from(err: HashError) -> Self {
        Self::Hash(err)
    }
}

impl From<JsonError> for AuthError {
    fn from(err: JsonError) -> Self {
        Self::Json(err)
    }
}

pub fn init_db(db: &mut Connection) -> Result<(), AuthError> {
    mod embedded {
        use refinery::embed_migrations;
        embed_migrations!("./src/migrations");
    }

    let report = embedded::migrations::runner()
        .set_abort_divergent(false)
        .run(db)
        .expect("Running database migrations failed, aborting.");
    log::info!(
        "Applied {} database migrations",
        report.applied_migrations().len()
    );

    Ok(())
}

/// Checks whether an username has been recently deleted
fn username_reserved(username: &str, db: &Connection) -> Result<bool, AuthError> {
    Ok(db
        .prepare("SELECT 1 FROM reserved_users WHERE username == ?1")?
        .exists(params![username])?)
}

fn user_exists(username: &str, db: &Connection) -> Result<bool, AuthError> {
    let mut stmt = db.prepare("SELECT uuid FROM users WHERE username == ?1")?;
    Ok(stmt.exists(params![username])?)
}

pub fn cleanup(db: &Connection) -> Result<(), AuthError> {
    let too_old = Utc::now() - chrono::Duration::days(USER_RESERVED_NAME_KEEP);

    db.execute(
        "DELETE FROM reserved_users WHERE reserved_at <= ?1",
        params![too_old.timestamp()],
    )?;

    Ok(())
}

#[allow(clippy::let_and_return)]
pub fn username_to_uuid(username_unfiltered: &str, db: &Connection) -> Result<Uuid, AuthError> {
    let username = decapitalize(username_unfiltered);
    let mut stmt = db.prepare_cached("SELECT uuid FROM users WHERE username == ?1")?;
    let result = stmt
        .query_map(params![&username], |row| row.get::<_, String>(0))?
        .filter_map(|s| s.ok())
        .find_map(|s| Uuid::parse_str(&s).ok())
        .or_else(|| {
            let uuid = db
                .query_row(
                    "SELECT uuid FROM reserved_users WHERE username = ?1",
                    params![&username],
                    |row| row.get::<_, String>(0),
                )
                .ok()?;

            Uuid::parse_str(&uuid).ok()
        })
        .ok_or(AuthError::UserDoesNotExist);
    result
}

#[allow(clippy::let_and_return)]
pub fn uuid_to_username(uuid: &Uuid, db: &Connection) -> Result<String, AuthError> {
    let uuid = uuid.as_simple().to_string();
    let mut stmt = db.prepare_cached("SELECT display_username FROM users WHERE uuid == ?1")?;
    let result = stmt
        .query_map(params![uuid], |row| row.get::<_, String>(0))?
        .find_map(|s| s.ok())
        .ok_or(AuthError::UserDoesNotExist);
    result
}

pub fn register(
    username_unfiltered: &str,
    password: &str,
    db: &Connection,
) -> Result<(), AuthError> {
    let username = decapitalize(username_unfiltered);

    if user_exists(&username, db)? {
        return Err(AuthError::UserExists);
    } else if username_reserved(&username, db)? {
        return Err(AuthError::UserReserved);
    }

    let uuid = Uuid::new_v4().as_simple().to_string();
    let pwhash = hash_password(password)?;
    db.execute(
        "INSERT INTO users (uuid, username, display_username, pwhash) VALUES(?1, ?2, ?3, ?4)",
        params![uuid, &username, username_unfiltered, pwhash],
    )?;
    Ok(())
}

pub fn change_password(
    ChangePasswordPayload {
        username,
        current_password,
        new_password,
    }: ChangePasswordPayload,
    db: &Connection,
) -> Result<(), AuthError> {
    let username = username.to_lowercase();

    if !is_valid(&username, &current_password, db)? {
        Err(AuthError::InvalidLogin)?
    }

    let uuid = username_to_uuid(&username, db)?.as_simple().to_string();
    let pwhash = hash_password(&new_password)?;

    db.execute(
        "UPDATE users SET pwhash = ?1 WHERE uuid = ?2",
        params![pwhash, uuid],
    )?;
    Ok(())
}

pub fn change_username(
    ChangeUsernamePayload {
        old_username,
        password,
        new_username,
    }: ChangeUsernamePayload,
    db: &mut Connection,
) -> Result<(), AuthError> {
    let old_username_decapitalized = old_username.to_lowercase();
    let new_username_decapitalized = new_username.to_lowercase();

    // Cannot change username if: credentials are invalid, new username already exists, new username was recently reserved
    if !is_valid(&old_username_decapitalized, &password, db)? {
        Err(AuthError::InvalidLogin)?
    } else if user_exists(&new_username_decapitalized, db)? {
        Err(AuthError::UserExists)?
    } else if username_reserved(&new_username_decapitalized, db)? {
        Err(AuthError::UserReserved)?
    }

    let uuid = username_to_uuid(&old_username, db)?.as_simple().to_string();

    // ... or this account already made an username change recently
    let now = Utc::now();
    let min_change_age = now - chrono::Duration::days(USER_CHANGE_INTERVAL);
    if db.query_row(
        "SELECT COUNT(1) FROM users WHERE (last_username_change <= ?1 OR last_username_change is null) AND uuid = ?2",
        params![min_change_age.timestamp(), uuid],
        |row| row.get::<usize, i32>(0),
    )? == 0
    {
        Err(AuthError::UserRecentlyChanged)?
    }

    let transaction = db.transaction()?;
    // Add an entry to reserved_users to ensure no new accounts with the old username can
    // be created and the old username resolves to the same uuid for a while
    assert_eq!(
        transaction.execute(
            "INSERT INTO reserved_users VALUES (?1, ?2, ?3)",
            params![&old_username_decapitalized, now.timestamp(), &uuid],
        )?,
        1
    );
    assert_eq!(
        transaction.execute(
            "UPDATE users SET username = ?1, display_username = ?2, last_username_change = ?3 WHERE uuid == ?4",
            params![
                &new_username_decapitalized,
                &new_username,
                now.timestamp(),
                uuid
            ],
        )?,
        1
    );

    transaction.commit()?;

    Ok(())
}

/// Checks if the password is correct and that the user exists.
#[allow(clippy::let_and_return)]
fn is_valid(username: &str, password: &str, db: &Connection) -> Result<bool, AuthError> {
    let mut stmt = db.prepare_cached("SELECT pwhash FROM users WHERE username == ?1")?;
    let result = stmt
        .query_map(params![&username], |row| row.get::<_, String>(0))?
        .filter_map(|s| s.ok())
        .find_map(|correct| argon2::verify_encoded(&correct, password.as_bytes()).ok())
        .ok_or(AuthError::InvalidLogin);
    result
}

/// Hashes a password using Argon2
fn hash_password(password: &str) -> Result<String, AuthError> {
    let hconfig = argon2::Config {
        variant: Variant::Argon2i,
        time_cost: 3,
        mem_cost: 4096,
        ..Default::default()
    };
    Ok(argon2::hash_encoded(
        password.as_bytes(),
        &salt(),
        &hconfig,
    )?)
}

pub fn generate_token(
    username_unfiltered: &str,
    password: &str,
    db: &Connection,
) -> Result<AuthToken, AuthError> {
    let username = decapitalize(username_unfiltered);
    if !is_valid(&username, password, db)? {
        return Err(AuthError::InvalidLogin);
    }

    let uuid = username_to_uuid(&username, db)?;
    let token = AuthToken::generate();
    TOKENS.insert(token, uuid);
    Ok(token)
}

pub fn delete_account(
    username_unfiltered: &str,
    password: &str,
    db: &mut Connection,
) -> Result<(), AuthError> {
    let username = decapitalize(username_unfiltered);
    if !is_valid(&username, password, db)? {
        return Err(AuthError::InvalidLogin);
    }
    let uuid = username_to_uuid(&username, db)?.as_simple().to_string();
    let transaction = db.transaction()?;
    transaction.execute(
        "DELETE FROM users WHERE uuid = ?1 AND username = ?2",
        params![uuid, &username],
    )?;

    let now = Utc::now().timestamp();
    transaction.execute(
        "INSERT INTO reserved_users VALUES (?1, ?2, ?3)",
        params![username, now, uuid],
    )?;

    transaction.commit()?;

    Ok(())
}

pub fn verify(token: AuthToken) -> Result<Uuid, AuthError> {
    let mut uuid = None;
    TOKENS.run(&token, |entry| {
        uuid = entry.map(|e| e.data);
        false
    });
    uuid.ok_or(AuthError::InvalidToken)
}

#[cfg(test)]
mod tests {
    use auth_common::{net_prehash, ChangePasswordPayload, ChangeUsernamePayload};
    use rusqlite::Connection;

    const USERNAME: &str = "Testing";
    const PASSWORD: &str = "hunter2";
    const NEW_USERNAME: &str = "Testing2";

    pub fn testing_db() -> Connection {
        let mut db = super::db().expect("Could not get testing database connection");
        super::init_db(&mut db).expect("Could not run migrations on testing database");
        db
    }

    pub fn testing_db_with_account() -> Connection {
        let db = testing_db();

        super::register(USERNAME, &net_prehash(PASSWORD), &db)
            .expect("Failed to create testing account");
        db
    }

    #[test]
    fn register_account() {
        let db = testing_db();

        super::register(USERNAME, &net_prehash(PASSWORD), &db)
            .expect("Account registration failed");
    }

    #[test]
    #[should_panic = "test failed expectedly"]
    fn register_duplicate_account() {
        let db = testing_db_with_account();

        super::register(&USERNAME.to_lowercase(), &net_prehash(PASSWORD), &db)
            .expect("test failed expectedly");
    }

    #[test]
    fn delete_account() {
        let mut db = testing_db_with_account();

        let prehashed_passord = net_prehash(PASSWORD);
        super::delete_account(USERNAME, &prehashed_passord, &mut db)
            .expect("Failed to delete testing account");
    }

    #[test]
    // Registering an account with a username of a just deleted account should fail
    #[should_panic = "test failed expectedly"]
    fn register_deleted() {
        let mut db = testing_db_with_account();

        let prehashed_passord = net_prehash(PASSWORD);
        super::delete_account(USERNAME, &prehashed_passord, &mut db)
            .expect("Failed to delete testing account");

        super::register(USERNAME, &prehashed_passord, &db).expect("test failed expectedly");
    }

    #[test]
    fn change_password() {
        const NEW_PASSWORD: &str = "hunter3";
        let db = testing_db_with_account();

        super::change_password(
            ChangePasswordPayload {
                username: USERNAME.to_string(),
                current_password: net_prehash(PASSWORD),
                new_password: net_prehash(NEW_PASSWORD),
            },
            &db,
        )
        .expect("Failed to change password");
        super::is_valid(&USERNAME.to_lowercase(), &net_prehash(NEW_PASSWORD), &db)
            .expect("New credentials arent valid");
    }

    #[test]
    fn change_username() {
        let mut db = testing_db_with_account();

        super::change_username(
            ChangeUsernamePayload {
                old_username: USERNAME.to_string(),
                password: net_prehash(PASSWORD),
                new_username: NEW_USERNAME.to_string(),
            },
            &mut db,
        )
        .expect("Failed to change username");
        super::is_valid(&NEW_USERNAME.to_lowercase(), &net_prehash(PASSWORD), &db)
            .expect("Could not log in with new username");
    }

    #[test]
    #[should_panic = "test failed expectedly"]
    fn register_changed_username() {
        let mut db = testing_db_with_account();

        super::change_username(
            ChangeUsernamePayload {
                old_username: USERNAME.to_string(),
                password: net_prehash(PASSWORD),
                new_username: NEW_USERNAME.to_string(),
            },
            &mut db,
        )
        .expect("Failed to change username");
        super::register(USERNAME, &net_prehash(PASSWORD), &db).expect("test failed expectedly");
    }

    /// Tests if old username resolves to the same uuid after renaming
    #[test]
    fn username_change_uuid_resolve() {
        let mut db = testing_db_with_account();

        super::change_username(
            ChangeUsernamePayload {
                old_username: USERNAME.to_string(),
                password: net_prehash(PASSWORD),
                new_username: NEW_USERNAME.to_string(),
            },
            &mut db,
        )
        .expect("Failed to change username");

        super::username_to_uuid(USERNAME, &db)
            .expect("Could not resolve UUID of previous username");
    }
}
