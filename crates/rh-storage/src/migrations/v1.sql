-- RemoteHub schema v3.
--
-- This script creates the entire database from scratch. In alpha mode
-- (current), schema bumps drop and recreate everything — there are no
-- incremental migrations. When we ship beta, this file becomes the
-- baseline and follow-up changes get their own files: v3.sql, v4.sql, ...
--
-- v2 (Stage 1.8) added to `hosts`: display_name, startup_command,
-- env_vars_json, detected_os. Alpha policy = drop & recreate, so the
-- bump from version 1 → 2 wipes the existing DB on next open.

-- Enforce foreign keys. SQLite has them OFF by default at the connection
-- level; the storage layer also runs `PRAGMA foreign_keys = ON` after
-- opening, but enabling it here makes the schema self-documenting.
PRAGMA foreign_keys = ON;

-- ---------------------------------------------------------------------
-- Meta: schema version marker. The migration runner reads this row to
-- decide whether the file on disk matches the version this binary
-- expects.
-- ---------------------------------------------------------------------
CREATE TABLE schema_meta (
    key     TEXT PRIMARY KEY NOT NULL,
    value   TEXT NOT NULL
);

INSERT INTO schema_meta (key, value) VALUES ('version', '10');

-- ---------------------------------------------------------------------
-- Host groups (hierarchical folders).
-- ---------------------------------------------------------------------
CREATE TABLE host_groups (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL,
    parent_id   TEXT REFERENCES host_groups(id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    UNIQUE (parent_id, name)
);

CREATE INDEX idx_host_groups_parent ON host_groups(parent_id);

-- ---------------------------------------------------------------------
-- Credentials metadata. Secrets themselves live in OS keychain;
-- this table holds only what we need to find and identify them.
-- ---------------------------------------------------------------------
CREATE TABLE credentials (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('password', 'ssh_key', 'ssh_key_agent')),
    username        TEXT NOT NULL DEFAULT '',
    keychain_ref    TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE (name)
);

CREATE INDEX idx_credentials_kind ON credentials(kind);

-- ---------------------------------------------------------------------
-- Hosts. `default_credential_id` is denormalized for cheap lookup;
-- the storage layer keeps it in sync with the `is_default` flag in
-- host_credentials.
-- ---------------------------------------------------------------------
CREATE TABLE hosts (
    id                      TEXT PRIMARY KEY NOT NULL,
    name                    TEXT NOT NULL,
    display_name            TEXT,
    group_id                TEXT REFERENCES host_groups(id) ON DELETE SET NULL,
    protocol                TEXT NOT NULL CHECK (protocol IN ('ssh', 'rdp')),
    hostname                TEXT NOT NULL,
    port                    INTEGER NOT NULL CHECK (port > 0 AND port < 65536),
    username                TEXT NOT NULL DEFAULT '',
    tags_json               TEXT NOT NULL DEFAULT '[]',
    color                   TEXT,
    notes                   TEXT,
    startup_command         TEXT,
    env_vars_json           TEXT NOT NULL DEFAULT '[]',
    detected_os             TEXT,
    default_credential_id   TEXT REFERENCES credentials(id) ON DELETE SET NULL,
    jump_host_id            TEXT,
    agent_forwarding        INTEGER NOT NULL DEFAULT 0,
    favorite                INTEGER NOT NULL DEFAULT 0,
    last_connected_at       TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);

CREATE INDEX idx_hosts_group ON hosts(group_id);
CREATE INDEX idx_hosts_protocol ON hosts(protocol);
CREATE INDEX idx_hosts_name ON hosts(name);

-- ---------------------------------------------------------------------
-- Host ↔ Credential many-to-many link.
-- ---------------------------------------------------------------------
CREATE TABLE host_credentials (
    host_id         TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    credential_id   TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
    is_default      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (host_id, credential_id)
);

CREATE INDEX idx_host_credentials_credential ON host_credentials(credential_id);

-- ---------------------------------------------------------------------
-- Settings as flat key/value. Values are JSON-encoded; schema is
-- enforced at the Rust layer (rh_core::settings::Settings).
-- ---------------------------------------------------------------------
CREATE TABLE settings (
    key     TEXT PRIMARY KEY NOT NULL,
    value   TEXT NOT NULL
);

-- ---------------------------------------------------------------------
-- Pinned SSH host keys (TOFU). Public material, identified by
-- (hostname, port). The session actor looks up the expected key during
-- the handshake; the user trusts unknown/changed keys explicitly.
-- ---------------------------------------------------------------------
CREATE TABLE known_hosts (
    hostname            TEXT NOT NULL,
    port                INTEGER NOT NULL,
    key_type            TEXT NOT NULL,
    fingerprint_sha256  TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    PRIMARY KEY (hostname, port)
);

CREATE TABLE rdp_known_certs (
    hostname            TEXT NOT NULL,
    port                INTEGER NOT NULL,
    fingerprint_sha256  TEXT NOT NULL,
    subject             TEXT NOT NULL,
    trusted_at          TEXT NOT NULL,
    PRIMARY KEY (hostname, port)
);

-- ---------------------------------------------------------------------
-- Per-record sync provenance + tombstones (sync engine, slice 2b).
-- One row per replicated record, keyed by (kind, id). `rev_wall` /
-- `rev_counter` are the two halves of the record's last-write HLC stamp;
-- `origin` is the device that produced it; `deleted = 1` marks a
-- tombstone (the entity row is gone, this row remains so the deletion
-- replicates). A single generic table keeps the entity schemas untouched.
-- ---------------------------------------------------------------------
CREATE TABLE sync_meta (
    kind        TEXT NOT NULL,
    id          TEXT NOT NULL,
    rev_wall    INTEGER NOT NULL,
    rev_counter INTEGER NOT NULL,
    origin      TEXT NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, id)
);
