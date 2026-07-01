//! SQLite connection pool + schema bootstrap.
//!
//! The server is a multi-tenant, opaque-blob store: `accounts` holds the
//! per-user login material (a password hash and/or a Yandex OAuth subject —
//! the latter wired in slice 3a-2, columns provisioned now), and `vaults`
//! holds exactly one E2E-encrypted blob per account with a monotonically
//! increasing `rev` used for optimistic concurrency. The server never sees
//! plaintext — `blob` is the client's sealed envelope, stored verbatim.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

pub async fn connect(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    init_schema(&pool).await?;
    Ok(pool)
}

async fn init_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS accounts (
            id              TEXT PRIMARY KEY NOT NULL,
            email           TEXT NOT NULL UNIQUE,
            password_hash   TEXT,
            yandex_sub      TEXT UNIQUE,
            email_verified  INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS vaults (
            account_id  TEXT PRIMARY KEY NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            blob        TEXT NOT NULL,
            rev         INTEGER NOT NULL,
            updated_at  TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // SSH ID: a public handle per account, resolvable at `/<handle>`, plus the
    // set of PUBLIC keys published under it. Public keys are NOT secret, so —
    // unlike the vault — they are stored in plaintext (the whole point is that
    // `curl https://host/<handle>` returns them). Private keys never leave the
    // client keychain.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS handles (
            account_id  TEXT PRIMARY KEY NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            handle      TEXT NOT NULL UNIQUE COLLATE NOCASE,
            created_at  TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS handle_keys (
            id          TEXT PRIMARY KEY NOT NULL,
            account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            key_type    TEXT NOT NULL,
            public_key  TEXT NOT NULL,
            label       TEXT,
            created_at  TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}
