# rh-sync-server

RemoteHub's self-hostable **sync backend**. An authenticated, multi-tenant,
versioned **opaque-blob** store: it keeps exactly one End-to-End-encrypted
vault envelope per account and serves it with optimistic concurrency. It
**never sees plaintext** — the master password that derives the vault key never
leaves the client. Finding or seizing this server yields only ciphertext.

Standalone crate (its own `[workspace]`); shares no code with the RemoteHub app.

## Run (Docker)

```bash
cd server
cp .env.example .env          # then edit: set a long random JWT_SECRET
docker compose up --build -d
curl http://localhost:8080/health      # -> ok
```

Data persists in the `sync-data` volume (`/data/sync.db`).

## Run (bare cargo)

```bash
cd server
JWT_SECRET=$(openssl rand -hex 32) DB_PATH=./sync.db cargo run --release
```

## Configuration (env)

| var | required | default | meaning |
|---|---|---|---|
| `JWT_SECRET` | **yes** | — | signs bearer tokens; long + random |
| `BIND_ADDR` | no | `0.0.0.0:8080` | listen address |
| `DB_PATH` | no | `/data/sync.db` | SQLite file (created if missing) |
| `TOKEN_TTL_HOURS` | no | `168` | token lifetime |
| `MAX_BLOB_BYTES` | no | `5242880` | max vault size |
| `PUBLIC_BASE_URL` | for OAuth | — | external origin, e.g. `https://pingie.ru`; builds the Yandex `redirect_uri` + verify links |
| `YANDEX_CLIENT_ID` | for OAuth | — | Yandex OAuth app id |
| `YANDEX_CLIENT_SECRET` | for OAuth | — | Yandex OAuth app secret (server-only; never shipped to clients) |
| `REQUIRE_EMAIL_VERIFICATION` | no | `false` | gate password login on a verified email |

## Yandex OAuth (slice 3a-2)

Desktop sign-in is **server-mediated** so the OAuth client secret stays here:
the app opens `GET /v1/oauth/yandex/start?cb=http://127.0.0.1:<port>/cb` in the
system browser → we 302 to Yandex → Yandex calls `/v1/oauth/yandex/callback` →
we exchange the code, read the user's id + email, upsert the account, mint a
bearer token, and 302 it back to the app's loopback `cb?token=…`.

Setup: register an app at `https://oauth.yandex.com/client/new`, platform
**Web services**, Redirect URI `https://pingie.ru/v1/oauth/yandex/callback`,
permissions `login:email` + `login:info`. Put the id/secret + `PUBLIC_BASE_URL`
in `.env`. OAuth is inert until all three are set.

## API

| method | path | auth | body / headers | result |
|---|---|---|---|---|
| `GET` | `/health` | — | — | `200 ok` |
| `POST` | `/v1/register` | — | `{ email, password }` | `201` · `409` if email taken |
| `POST` | `/v1/login` | — | `{ email, password }` | `{ token }` · `401` |
| `GET` | `/v1/vault` | Bearer | — | `200 { blob_b64, rev }` · `204` if empty |
| `PUT` | `/v1/vault` | Bearer | `{ blob_b64 }` + `If-Match: <rev>` (omit to create) | `200 { rev }` · `409` stale · `412` create-collision |

`rev` is an opaque version token (compare by equality). The RemoteHub client's
`ServerRemote` maps `409` → re-pull + re-merge + retry. `blob_b64` is the
client's sealed export string, stored verbatim.

## TLS

Terminate TLS in front of this (Caddy / Cloudflare / nginx). Example Caddy:

```
sync.example.com {
    reverse_proxy localhost:8080
}
```

The default RemoteHub endpoint should be a domain behind such a proxy so the
origin IP stays hidden (see `docs/specs/sync.md` §9.4).

## Roadmap

- **3a-2:** Yandex OAuth (`/v1/oauth/yandex/{start,callback}`) + email
  verification. The `accounts` table already has `yandex_sub` and
  `email_verified` columns for this.
- The RemoteHub client side (`ServerRemote` + `sync_now`) lands as slice 3b.
