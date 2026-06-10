# Auto-updater (self-hosted on pingie.ru)

RemoteHub updates itself via the Tauri 2 updater plugin. The app checks a static
`latest.json` on our own server, downloads a signed NSIS installer, verifies the
signature with a baked-in public key, and applies it on restart.

```
dev PC  ──build+sign──>  setup.exe + .sig
        ──release.ps1──>  latest.json + setup.exe  ──scp──>  pingie.ru:/srv/remotehub-updates/
client  ──GET──────────>  https://pingie.ru/updates/latest.json  ──download+verify+install──>  restart
```

Nothing secret lives on the server — only the public installer and a manifest
whose signature is verified by the **public** key shipped in the app.

---

## 1. One-time: signing keypair

The updater refuses unsigned updates (cannot be disabled). Generate a keypair
**once** and keep the private key safe forever — lose it and you can never push
updates to already-installed clients.

```powershell
# from the repo root (uses the Tauri CLI)
cargo tauri signer generate -w $env:USERPROFILE\.tauri\remotehub.key
# -> prints the PUBLIC key and writes the private key (password-protected)
```

- Private key: `%USERPROFILE%\.tauri\remotehub.key` — **never commit, keep a backup.**
- Public key: paste the printed value (the `.key.pub` contents) into
  `crates/rh-app/tauri.conf.json` → `plugins.updater.pubkey`, replacing the
  `REPLACE_WITH_CONTENTS_OF_remotehub.key.pub` placeholder.

Until the real pubkey is in place, `check()` will just fail silently (no crash),
so the app still runs — but no updates will verify.

## 2. One-time: nginx on pingie.ru

Files live on the VPS filesystem and are served as plain static by the existing
nginx (same box as the sync server, TLS already terminated there).

```bash
sudo mkdir -p /srv/remotehub-updates
```

Add inside the `pingie.ru` server block:

```nginx
location /updates/ {
    alias /srv/remotehub-updates/;
    # latest.json must never be cached stale; installers are immutable.
    location = /updates/latest.json {
        alias /srv/remotehub-updates/latest.json;
        add_header Cache-Control "no-store";
    }
}
```

```bash
sudo nginx -t && sudo systemctl reload nginx
```

SSH key-based access for your dev PC so `scp` in `release.ps1` is non-interactive.

## 3. Releasing a new version

```powershell
# from the repo root
.\release.ps1 -Version 0.2.1 -Notes "Port-forward redesign; terminal pop-out"
```

The script:
1. Bumps the version in `Cargo.toml` (workspace), `tauri.conf.json`, `ui/package.json`.
2. Builds the signed installer (`cargo tauri build`, prompts for the key password).
3. Writes `latest.json` (version + url + signature, Tauri static format).
4. `scp`s `*-setup.exe` + `latest.json` to `pingie.ru:/srv/remotehub-updates/`.

Then `git commit` the version bump.

## 4. How the client behaves

- **On launch:** silent check. If newer, it downloads in the background and shows
  a slim "Update vX ready — restart to apply" bar. No interruption.
- **Settings → About:** a "Check for updates" button + status; same restart bar.
- **Restart** runs the NSIS installer in `passive` mode (no buttons) and relaunches
  into the new version.

## 5. Gotchas

- **Install from `setup.exe`, not `cargo tauri dev`.** The updater replaces an
  installed (NSIS) app; a dev/portable run won't update.
- The three version strings (Cargo workspace / tauri.conf.json / package.json)
  must match — `release.ps1` keeps them in sync.
- `bundle.createUpdaterArtifacts: true` means `cargo tauri build` **requires** the
  signing env vars (`TAURI_SIGNING_PRIVATE_KEY` + `_PASSWORD`); `release.ps1` sets
  them. Plain `cargo tauri dev` is unaffected.
- The updater's HTTP/download is done by the Rust side (reqwest), so the webview
  CSP does not need a `connect-src` entry for pingie.ru.
