#requires -Version 5.0
<#
.SYNOPSIS
    Dev helper for RemoteHub on Windows.

.DESCRIPTION
    Wraps common dev tasks so we don't fight PowerShell quoting around
    Tauri / pnpm / cargo invocations.

.PARAMETER Mode
    One of:
      dev       — runs `cargo tauri dev` (UI + Rust hot-reload)
      build     — production bundle
      test      — `cargo test --workspace`
      lint      — `cargo clippy + cargo fmt --check + pnpm lint`
      icons     — regenerate icon set from design/source-icon.png
      clean     — `cargo clean` + remove ui/dist + ui/node_modules

.EXAMPLE
    pwsh -NoProfile -File scripts/dev.ps1 -Mode dev
#>

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("dev", "build", "test", "lint", "icons", "clean")]
    [string]$Mode
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

function Invoke-InRoot {
    param([string]$Command)
    Push-Location $root
    try {
        Write-Host "> $Command" -ForegroundColor Cyan
        Invoke-Expression $Command
    }
    finally {
        Pop-Location
    }
}

switch ($Mode) {
    "dev" {
        Invoke-InRoot "cargo tauri dev"
    }
    "build" {
        Invoke-InRoot "cargo tauri build"
    }
    "test" {
        Invoke-InRoot "cargo test --workspace"
    }
    "lint" {
        Invoke-InRoot "cargo fmt --all -- --check"
        Invoke-InRoot "cargo clippy --workspace --all-targets -- -D warnings"
        Push-Location (Join-Path $root "ui")
        try {
            Write-Host "> pnpm lint" -ForegroundColor Cyan
            pnpm lint
        }
        finally {
            Pop-Location
        }
    }
    "icons" {
        $src = Join-Path $root "design/source-icon.png"
        if (-not (Test-Path $src)) {
            Write-Error "design/source-icon.png not found. Create a 1024x1024 PNG there first."
        }
        Invoke-InRoot "cargo tauri icon `"$src`""
    }
    "clean" {
        Invoke-InRoot "cargo clean"
        $dist = Join-Path $root "ui/dist"
        $nodemod = Join-Path $root "ui/node_modules"
        if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
        if (Test-Path $nodemod) { Remove-Item -Recurse -Force $nodemod }
        Write-Host "Cleaned." -ForegroundColor Green
    }
}
