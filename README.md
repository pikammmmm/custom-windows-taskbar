# glassbar

A glassy, always-on-top floating dock + HUD for Windows. Coexists with the auto-hidden Windows taskbar.

[![Latest release](https://img.shields.io/github/v/release/pikammmmm/glassbar?label=download)](https://github.com/pikammmmm/glassbar/releases/latest)

## Features

- **Dock** — pinned + running app icons, click to launch / focus / minimize, right-click for window list + pin / unpin, drag to reorder.
- **Spotlight launcher** — Win-key tap or the dock's launcher button opens a glassy search overlay. Indexes the Start Menu, UWP / Microsoft Store apps (via `Get-StartApps`), and files in your common folders. Acronym + fuzzy / typo-tolerant matching.
- **HUD** — clock, ARSO Maribor weather, CPU / RAM / network throughput, now-playing media, audio device switcher, file stash with native drag-out, Cloudflare WARP toggle, system power menu, Claude 5-hour usage bar.
- **Glass** — Win11 acrylic backdrop, layered transparency, hand-drawn SVG icons for system apps.

## Install

Easiest path is the prebuilt MSI from the [Releases page](https://github.com/pikammmmm/glassbar/releases/latest):

1. Download `glassbar_<version>_x64_en-US.msi`.
2. Run it — installs to `Program Files\glassbar\` and adds a Start-menu entry.
3. Launch from Start. The dock auto-shows when your cursor reaches the bottom of the screen.
4. (Optional) Open the HUD → **Settings** → toggle *Launch at sign-in* for autostart.

To uninstall: Settings → Apps → Installed apps → glassbar → Uninstall.

## Build from source

Requires Rust (stable). The WiX Toolset is needed for the MSI bundle and ships preinstalled on `windows-latest` GitHub runners.

```bash
cargo install tauri-cli --version "^2.0"
cd src-tauri
cargo tauri build
```

The MSI installer is written to `src-tauri/target/release/bundle/msi/`.

## Releasing

Tag a commit with `vX.Y.Z` and push the tag. `.github/workflows/release.yml` runs `cargo tauri build` on a Windows runner and attaches the MSI to a GitHub Release automatically:

```bash
git tag v0.1.0
git push origin v0.1.0
```

A manual run is also wired in via the workflow's *Run workflow* button — that leaves the MSI as a workflow artifact instead of publishing a release.

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
