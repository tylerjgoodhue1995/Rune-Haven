-- Pre-migration system tables for compatibility
CREATE TABLE IF NOT EXISTS users (
    uuid             TEXT NOT NULL PRIMARY KEY,
    username         TEXT NOT NULL UNIQUE,
    display_username TEXT NOT NULL UNIQUE,
    pwhash           TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS deleted_users (
    username   TEXT PRIMARY KEY,
    deleted_at INT  NOT NULL
);

ALTER TABLE users          ADD COLUMN    last_username_change TEXT;
ALTER TABLE deleted_users  RENAME TO     reserved_users;
ALTER TABLE reserved_users RENAME COLUMN deleted_at TO reserved_at;
ALTER TABLE reserved_users ADD COLUMN    uuid TEXT;
