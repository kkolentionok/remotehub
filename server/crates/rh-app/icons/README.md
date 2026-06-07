# Icons

Placeholder icons generated during Stage 1.1 scaffolding. All files are valid
in their respective formats but visually trivial (solid blue 32×32 / 128×128).

Files:

- `32x32.png`, `128x128.png`, `128x128@2x.png` — used by Tauri on Linux/macOS
  and embedded into bundle resources.
- `icon.ico` — required by `tauri-build` on Windows even in dev mode (the
  build script embeds it as a Windows Resource into the .exe). Cannot be
  omitted or generation fails with: `icons/icon.ico not found`.
- `icon.icns` — required for macOS bundle (`.app`). Not needed for dev runs
  on Windows but kept here for completeness.

To replace with a real icon set when design is ready, drop a square source
PNG (recommended 1024×1024 with transparent background) somewhere — for
example `design/source-icon.png` — and run:

```bash
cargo tauri icon ./design/source-icon.png
```

This overwrites all files above with properly-sized variants.
