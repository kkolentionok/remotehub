# Sync & Portable Vault — RemoteHub

Status: **foundation implemented** (`crates/rh-vault`), backend **not yet chosen** (A/B/C — see "Backend decision"). This document is the design contract for backlog item 2 (accounts + authorization + sync).

## 1. Goals & non-goals

**Goals**
- A **portable, end-to-end-encrypted vault**: the user's hosts, groups, credentials (with secrets) and settings, sealed under a master password, exportable to a single file and importable on another device.
- **Multi-device sync** of that state across a user's own machines, **convergent** (every device ends in the same state) and **offline-tolerant** (edits made while disconnected merge correctly later).
- **Backend-agnostic core**: the crypto, data model and merge must not depend on *where* the encrypted blob is stored. The storage backend is a single pluggable seam.

**Non-goals (v1)**
- Real-time collaboration / presence. Sync is store-and-forward (pull → merge → push).
- Server-side knowledge of plaintext. The backend only ever sees ciphertext (this is what "E2E" means here).
- Sharing a subset of hosts with *another user* (team ACLs). The "Team" storage scope in the UI is a forward-looking seam; v1 targets one user, many devices.
- Per-field merge (see §6 — record-level LWW for v1, field-level is the planned v2 refinement).

## 2. Threat model — what is and isn't protected

The bytes that leave the device are **AES-256-GCM ciphertext**. An attacker who obtains the blob (a compromised server, a snooped cloud-sync folder, a stolen export file) learns:
- the **size** of the encrypted state (roughly: how many hosts/credentials exist), and
- the **KDF parameters and salt** (cleartext header — they must be, to re-derive the key).

They do **not** learn any host name, address, username, note, secret, or setting, and they **cannot** tamper with the blob undetected: the cleartext header is bound into the AEAD as additional authenticated data (AAD), so flipping a bit in the ciphertext *or* downgrading the KDF parameters makes the authentication tag fail to verify.

The master password never leaves the device. A wrong password and a corrupted blob are **cryptographically indistinguishable** — both surface as a single `Decrypt` error, by design (no oracle for "the password was close").

Out of scope: a compromised endpoint (keylogger, malware with the vault unlocked in memory). Local secrets continue to live in the OS keychain; the vault is the *sync/export* representation, assembled in memory only when sealing.

## 3. Crate layout

```
rh-app  →  rh-vault  →  rh-core
```

`rh-vault` is a new, dependency-light, backend-agnostic crate. It contains **no** Tauri, SQLite, keychain, or network code. Modules:

| module        | responsibility                                                   |
|---------------|------------------------------------------------------------------|
| `clock`       | `Hlc` hybrid logical clock + `HlcGenerator` + `NodeId`           |
| `kdf`         | Argon2id password → `VaultKey`; `KdfParams` (cleartext header)   |
| `crypto`      | AES-256-GCM `seal` / `open` (RustCrypto `aes-gcm`)               |
| `model`       | `SyncRecord`, `RecordMeta`, `SyncSnapshot`, `EntityKind`         |
| `merge`       | record-level LWW reconciliation of two snapshots                 |
| `envelope`    | `VaultEnvelope` — the portable sealed form + export string       |
| `transport`   | `SyncRemote` trait (the A/B/C seam) + `MemoryRemote` test double  |
| `error`       | `VaultError` (thiserror; no secrets in any variant)              |
| `b64`/`opt_b64` | serde adapters: bytes ⇄ base64 in the JSON forms               |

`rh-app` is responsible for everything stateful: reading entities from storage to build a snapshot, pulling secrets from the keychain into `SyncCredentialPayload`, persisting the `NodeId` and the clock seed, choosing a concrete transport, and writing a merged snapshot back into storage + keychain.

## 4. Cryptography

### 4.1 Key derivation — Argon2id

Master password is stretched to a 256-bit key with **Argon2id** (memory-hard; resists GPU/ASIC brute force). Parameters (`KdfParams`) are stored in the cleartext envelope header so a future build can re-derive the same key:

| param        | default            | note                              |
|--------------|--------------------|-----------------------------------|
| `m_cost_kib` | 65536 (= 64 MiB)   | OWASP "first recommended" profile |
| `t_cost`     | 3                  | iterations                        |
| `p_cost`     | 1                  | lanes                             |
| `salt`       | 16 random bytes    | per-vault, from the OS CSPRNG     |

Each vault records the parameters it was sealed with, so the policy can be raised later without invalidating old vaults.

**Why the `argon2` crate, not aws-lc-rs:** aws-lc-rs 1.17 does not expose Argon2id in its public Rust API. The RustCrypto `argon2` crate is used *only* for this derivation, mirroring the existing scoped use in `rh-ssh/ppk.rs`. It is already in `Cargo.lock`.

### 4.2 Payload encryption — AES-256-GCM

The serialized snapshot is sealed with **AES-256-GCM** via the pure-Rust RustCrypto `aes-gcm` crate. A fresh random 96-bit nonce is generated for **every** seal and stored next to the ciphertext. The vault is sealed seldom (on save/sync), well clear of the random-nonce birthday bound; even so, any re-seal gets a new nonce, and a nonce is never reused with the same key.

> **Why `aes-gcm`, not aws-lc-rs:** the original plan was aws-lc-rs, but its native `aws-lc-sys` C library needs NASM + a C11 MSVC toolchain to *build* on Windows (it was only ever a `Cargo.lock` entry before — russh/rustls use `ring`, so it never compiled). `aes-gcm` is pure Rust (hardware AES-NI via the `aes` crate when present), well-audited, and already in the dependency tree, so it adds no native build step. To restore aws-lc-rs instead, install NASM and set `AWS_LC_SYS_PREBUILT_NASM=1`.

The GCM tag (16 bytes) is appended to the ciphertext. AAD = canonical JSON of `{format, kdf}` — binding the header so an attacker can't swap in weaker KDF params and still verify.

## 5. Data model — what syncs

Four `EntityKind`s replicate: `Host`, `Group`, `Credential`, `Setting`. Each is a **`SyncRecord`**:

```rust
struct SyncRecord {
    kind: EntityKind,
    id: String,                  // entity ULID, or the setting key
    meta: RecordMeta,
    data: Option<serde_json::Value>,   // None ⟺ tombstone
}

struct RecordMeta {
    rev: Hlc,                    // logical time of the last write
    origin: NodeId,             // who wrote it (deterministic tiebreaker)
    deleted: bool,              // tombstone flag
    field_revs: BTreeMap<String, Hlc>,  // RESERVED for field-level LWW (v2); empty in v1
}
```

### 5.1 Opaque payloads

A record's `data` is the entity serialized to `serde_json::Value`, **not** a hand-maintained shadow struct. Adding a field to `rh_core::Host` automatically flows through sync with no change to `rh-vault`. The trade-off (machine-set fields riding along) is documented in §8.

### 5.2 Secrets

Credential secrets live in the OS keychain locally and are never written to SQLite. For sync they are carried inside the credential record as `SyncCredentialPayload { credential, secret, passphrase }`, where `secret`/`passphrase` are raw bytes (base64 in the JSON). These plaintext bytes exist **only inside the encrypted envelope** — on the wire they are GCM ciphertext. (Covered by an end-to-end test that asserts the secret never appears, plain or base64, in the exported blob.)

### 5.3 Snapshot

```rust
struct SyncSnapshot {
    format: u32,
    node: NodeId,        // producing device
    generated: Hlc,      // highest stamp emitted/observed at production time
    records: Vec<SyncRecord>,
}
```

The snapshot is the plaintext sealed into the envelope, and the unit the merge consumes. `generated` lets a receiver fold the sender's clock into its own (`HlcGenerator::observe`) so its future stamps sort after anything it has seen.

## 6. Conflict resolution

### 6.1 Logical time — Hybrid Logical Clock

Wall-clock timestamps are unsafe across devices with skewed clocks (a later edit on a slow clock could lose to an earlier edit). We stamp every write with an **HLC**: `(wall_ms, counter)`, monotonic, never regressing — if the physical clock stalls or jumps backwards, the counter advances instead. On importing a remote snapshot, the local generator `observe`s the remote `generated` clock so subsequent local stamps strictly follow it.

### 6.2 Record-level last-write-wins (v1)

Merge takes the union of records keyed by `(kind, id)`. Where both sides hold the same key, the winner is the record whose `(rev, origin)` is greater — a **total, deterministic** order (ties at an identical `rev` are broken by the lexicographically greater `NodeId`). Because the order is total and deterministic, the merge is **commutative, idempotent, and convergent**: every device that sees the same inputs computes the identical result regardless of merge order.

Tombstones are ordinary records (`deleted: true`, `data: None`), so a delete competes with an edit on the same order:
- delete **after** edit → entity stays deleted;
- edit **after** delete → entity comes back ("undelete").

Tombstones are **retained** through merges (they must keep propagating until every replica has applied them). Pruning is a separate concern — see §8.

### 6.3 Field-level LWW (planned v2)

The one cost of record-level LWW: two devices editing *different fields* of the *same* record concurrently — the higher-`rev` record wins wholesale and the other field-edit is lost. For RemoteHub's reality (one user, 2–3 devices, hosts edited rarely) this is uncommon. The refinement is per-field stamps in `RecordMeta.field_revs`, already reserved in the format (empty in v1), so the upgrade is **additive** — old snapshots stay readable, new ones simply populate the map. No format break.

## 7. Transport — the A/B/C seam

The transport moves **opaque ciphertext** and never sees plaintext:

```rust
#[async_trait]
trait SyncRemote: Send + Sync {
    async fn pull(&self) -> Result<Option<RemoteBlob>, VaultError>;          // None = remote empty
    async fn push(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, VaultError>;
}
struct RemoteBlob { bytes: Vec<u8>, version: String }   // version = opaque token
```

**Optimistic concurrency** is the contract: every stored blob carries a version token; `push` takes the version the caller merged against; if the remote moved since, push fails with `RemoteConflict` and the engine pulls → re-merges → retries. This prevents lost updates when two devices sync near-simultaneously. (Covered by `concurrent_push_forces_remerge`.)

The sync loop in `rh-app` is written **once** against this trait:

```
pull → (open envelope) → merge with local snapshot → (seal) → push(expected = pulled version)
   └── on RemoteConflict: pull again, re-merge, retry
```

### How each backend satisfies the contract

| | backend | `version` token | `push` mechanism |
|---|---|---|---|
| **A** | self-hosted server | server row revision | `If-Match: <rev>`; server 409s a stale rev |
| **B** | object store (S3 / WebDAV) | ETag | conditional `PUT If-Match` (S3) / `If` precondition (WebDAV) |
| **C** | file in a cloud-sync folder (OneDrive/Dropbox) | content hash + mtime | re-read before write & compare; OS client handles actual transport |

## 8. Known semantics & limitations (v1)

- **Machine-set fields ride along.** Because payloads are opaque, `Host.detected_os` and `Host.last_connected_at` (set locally by the machine, not the user) are part of the record and follow record-level LWW like any field. A device that connected most recently will, on its next write, carry its `last_connected_at` to peers. Acceptable for v1; a candidate for field-level treatment in v2 (these fields are natural "max-wins" merges).
- **`rh-core` `Option` fields are not `#[serde(default)]`.** Stripping unknown/machine fields and re-deserializing would fail. The opaque-`Value` approach sidesteps this — we never partially reconstruct an entity; we round-trip the whole JSON.
- **Tombstone GC is deferred.** Tombstones accumulate. A later pass can drop tombstones older than a safe horizon (e.g. older than the oldest device's last-seen clock), once we track per-device watermarks. Not needed at v1 scale.
- **Settings are last-write-wins per key** like any record (`id` = setting key). No structural merge of a setting's JSON value.
- **`NodeId` provisioning** is `rh-app`'s job: assign a fresh ULID per device on first run and persist it; persist the clock seed (`HlcGenerator::last`) so restarts don't regress.

## 9. Backend decision (OPEN — awaiting user)

Recommendation order, given the foundation is transport-agnostic (the choice only changes one trait impl):

1. **C — cloud-sync folder file (recommended default).** Fastest to ship, zero infrastructure, validates the entire pipeline (seal → merge → open) immediately. The OS sync client (OneDrive/Dropbox) handles transport and even offline queueing; our optimistic-concurrency check + merge handles its occasional "conflicted copy".
2. **B — object store (S3/WebDAV) next.** No dependency on a desktop sync client; real conditional-PUT concurrency; works headless. The natural step once sync is proven.
3. **A — self-hosted server only if hosted team sync becomes a goal.** The "Team" storage scope hints at this future, but it's the most infrastructure for the least immediate return.

This is the **one open question** for the user. Everything in §3–§7 is independent of the answer and is implemented now.

## 10. Status

- `crates/rh-vault` — implemented with unit + integration tests (`cargo test -p rh-vault`). Verified by tests, **not yet compiled on the user's toolchain** (sandbox has no cargo).
- Crypto now builds clean on the user's Windows toolchain: AES-256-GCM via RustCrypto `aes-gcm` (pure Rust, no native lib), Argon2id via `argon2`, RNG via `getrandom`. The earlier aws-lc-rs route was dropped because `aws-lc-sys` needs NASM + C11 to build (see §4.2). Argon2 `Params::new(m,t,p,Some(32))` + `hash_password_into` confirmed.

## 11. Next (after compile + backend choice)

- IPC commands: `vault_export` / `vault_import` / `vault_status` in `rh-app`.
- Frontend: master-password UX (set/unlock), export/import UI, wire the **Team** storage scope (`TabBar.tsx`, `storage.*` keys) to "sync configured".
- A concrete `SyncRemote` impl for the chosen backend, plus the `rh-app` sync engine (snapshot build from storage + keychain; merged snapshot write-back).
