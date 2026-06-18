<#
.SYNOPSIS
  Build, sign and publish a RemoteHub update to the self-hosted endpoint.

.DESCRIPTION
  One command per release. Steps:
    1. Bump the version in the three sources of truth (workspace Cargo.toml,
       crates/rh-app/tauri.conf.json, ui/package.json).
    2. Build the signed NSIS installer (createUpdaterArtifacts -> *-setup.exe + .sig).
    3. Generate latest.json (version + url + signature) in the Tauri static format.
    4. scp the installer + latest.json to the VPS, where nginx serves them at
       https://<Server>/updates/.

  Run from the repo root:  .\release.ps1 -Version 0.2.1 -Notes "what changed"

.NOTES
  Prereqs (one-time setup, see docs/UPDATER.md):
    - A signing keypair (tauri signer generate); the public key sits in
      tauri.conf.json -> plugins.updater.pubkey.
    - SSH access to the VPS (key-based) so scp is non-interactive.
  NEVER commit the private key. It lives only on this machine + a safe backup.
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Notes = "",
    [string]$Server = "dl.pingie.ru",
    [string]$UploadHost = "",
    [int]$Port = 22,
    [string]$SshUser = "root",
    [string]$RemoteDir = "/srv/remotehub-updates",
    [string]$KeyPath = "$env:USERPROFILE\.tauri\remotehub.key",
    # Stable "download latest" filename, overwritten every release. The updater
    # manifest keeps pointing at the versioned exe (immutable); this is just a
    # fixed human link: https://<Server>/updates/<StableName>
    [string]$StableName = "pingiesetup.exe",
    # Standalone (no-installer) build: the raw exe, uploaded both versioned and
    # under this stable name. Same binary, just run without installing.
    [string]$LightStableName = "light.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

# When the public domain is behind a CDN (Cloudflare), SSH/scp must target the
# real origin host, while the download URL stays on the domain. Default the
# upload host to the domain when no separate origin is given.
if (-not $UploadHost) { $UploadHost = $Server }

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Version must be semver, e.g. 0.2.1 (got '$Version')"
}

Write-Host "==> Bumping version to $Version" -ForegroundColor Cyan

# 1) Workspace Cargo.toml: only the standalone `version = "x"` line (under
#    [workspace.package]); dependency versions are `name = { version = ... }`
#    and are not at line start, so the ^ anchor leaves them alone.
(Get-Content Cargo.toml -Raw) `
    -replace '(?m)^version = "[^"]*"', "version = `"$Version`"" |
    Set-Content Cargo.toml -NoNewline

# 2) tauri.conf.json
$conf = "crates\rh-app\tauri.conf.json"
(Get-Content $conf -Raw) `
    -replace '"version":\s*"[^"]*"', "`"version`": `"$Version`"" |
    Set-Content $conf -NoNewline

# 3) ui/package.json (top-level "version" only; dep keys are package names)
$pkg = "ui\package.json"
(Get-Content $pkg -Raw) `
    -replace '"version":\s*"[^"]*"', "`"version`": `"$Version`"" |
    Set-Content $pkg -NoNewline

# --- Build (signed) -------------------------------------------------------
Write-Host "==> Building signed bundle" -ForegroundColor Cyan
if (-not (Test-Path $KeyPath)) {
    throw "Signing key not found at $KeyPath. Run 'tauri signer generate' first (see docs/UPDATER.md)."
}
$securePw = Read-Host "Signing key password" -AsSecureString
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $KeyPath -Raw)
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD =
    [Runtime.InteropServices.Marshal]::PtrToStringAuto(
        [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePw))

cargo tauri build
if ($LASTEXITCODE -ne 0) { throw "cargo tauri build failed" }

# --- Locate artifacts -----------------------------------------------------
$nsisDir = "target\release\bundle\nsis"
$exe = Get-ChildItem "$nsisDir\*-setup.exe" -ErrorAction Stop |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
$sigFile = "$($exe.FullName).sig"
if (-not (Test-Path $sigFile)) { throw "Signature not found: $sigFile" }
$signature = (Get-Content $sigFile -Raw).Trim()

Write-Host "==> Installer: $($exe.Name)" -ForegroundColor Green

# Standalone (no-installer) binary straight out of target/release. cargo emits
# it under the package name (rh-app) but Tauri renames it to productName
# (RemoteHub); accept either.
$lightExe = Get-ChildItem (Join-Path $root "target\release\*.exe") -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -in @("RemoteHub.exe", "rh-app.exe") } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
$lightVersionedName = "RemoteHub_${Version}_x64-light.exe"

# --- latest.json (Tauri static format) ------------------------------------
$pubDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$manifest = [ordered]@{
    version   = $Version
    notes     = $Notes
    pub_date  = $pubDate
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = $signature
            url       = "https://$Server/updates/$($exe.Name)"
        }
    }
}
$json = $manifest | ConvertTo-Json -Depth 6
# UTF-8 *without* BOM: Set-Content -Encoding utf8 emits a BOM on Windows
# PowerShell, and the updater's JSON parser (serde_json) rejects a leading BOM.
[System.IO.File]::WriteAllText(
    (Join-Path $root "latest.json"),
    $json,
    (New-Object System.Text.UTF8Encoding($false)))
Write-Host "==> Wrote latest.json (v$Version)" -ForegroundColor Green

# --- Upload ---------------------------------------------------------------
Write-Host "==> Uploading to ${SshUser}@${UploadHost}:${RemoteDir} (port $Port)" -ForegroundColor Cyan
scp -P $Port $exe.FullName "${SshUser}@${UploadHost}:${RemoteDir}/"
if ($LASTEXITCODE -ne 0) { throw "scp installer failed" }
scp -P $Port "latest.json" "${SshUser}@${UploadHost}:${RemoteDir}/"
if ($LASTEXITCODE -ne 0) { throw "scp manifest failed" }

# Stable "download latest" copy (same signed bytes, fixed name; overwritten each release).
scp -P $Port $exe.FullName "${SshUser}@${UploadHost}:${RemoteDir}/$StableName"
if ($LASTEXITCODE -ne 0) { throw "scp stable installer failed" }

# Standalone (no-installer) build: versioned + stable name. Non-fatal if the
# raw exe wasn't found, so a failure here never blocks the real release.
if ($lightExe) {
    scp -P $Port $lightExe.FullName "${SshUser}@${UploadHost}:${RemoteDir}/$lightVersionedName"
    if ($LASTEXITCODE -ne 0) { throw "scp light (versioned) failed" }
    scp -P $Port $lightExe.FullName "${SshUser}@${UploadHost}:${RemoteDir}/$LightStableName"
    if ($LASTEXITCODE -ne 0) { throw "scp light (stable) failed" }
} else {
    Write-Host "==> WARN: standalone exe not found in target/release; skipped light upload" -ForegroundColor Yellow
}

Write-Host "==> Published v$Version -> https://$Server/updates/latest.json" -ForegroundColor Green
Write-Host "    Versioned: https://$Server/updates/$($exe.Name)" -ForegroundColor Green
Write-Host "    Stable:    https://$Server/updates/$StableName" -ForegroundColor Green
if ($lightExe) {
    Write-Host "    Light:     https://$Server/updates/$lightVersionedName" -ForegroundColor Green
    Write-Host "    Light std: https://$Server/updates/$LightStableName" -ForegroundColor Green
}
Write-Host "    Don't forget: git commit the version bump." -ForegroundColor Yellow
