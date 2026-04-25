# glassbar

A glassy, always-on-top floating dock + HUD for Windows. Coexists with the auto-hidden Windows taskbar.

## Features

- **Dock** — pinned + running app icons, click to launch/focus/minimize, right-click for window list and pin/unpin.
- **HUD** — clock, network throughput, now-playing media. Drag to reposition.
- **Glass** — Mica on Windows 11, Acrylic on Windows 10.

## Build

```bash
cargo install tauri-cli --version "^2.0"
cargo tauri build
```

The MSI installer is written to `src-tauri/target/release/bundle/msi/`.

## Configuration

Files live in `%APPDATA%\glassbar\glassbar\data\`:

- `pinned.json` — array of `{ "path", "display_name", "icon_path"? }`. Hot-reloaded on save.
- `settings.json` — `{ "hud_position": [x, y]?, "auto_start": bool }`.

A starter `pinned.example.json` ships in the repo.

## Auto-start at login

Toggle from the dock devtools (or call `set_autostart` programmatically). Disabled by default.

## Requirements

- Windows 10 (1809+) or Windows 11
- WebView2 runtime (preinstalled on Win11)

## Limitations (v1)

- Single primary monitor only.
- No third-party system tray hosting (planned).
- No in-app settings UI — edit JSON for now.
- One glass theme.
