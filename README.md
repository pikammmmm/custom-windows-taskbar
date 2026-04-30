# glassbar

A glassy, always-on-top floating dock + HUD for Windows. Coexists with the auto-hidden Windows taskbar.

[![Latest release](https://img.shields.io/github/v/release/pikammmmm/glassbar?label=download)](https://github.com/pikammmmm/glassbar/releases/latest)

## Features

- **Dock** — pinned + running app icons, click to launch / focus / minimize, right-click for window list + pin / unpin, drag to reorder.
- **Spotlight launcher** — Win-key tap or the dock's launcher button opens a glassy search overlay. Indexes the Start Menu, UWP / Microsoft Store apps (via `Get-StartApps`), and files in your common folders. Acronym + fuzzy / typo-tolerant matching.
- **HUD** — clock, ARSO Maribor weather, CPU / RAM / network throughput, now-playing media, audio device switcher, file stash with native drag-out, Cloudflare WARP toggle, system power menu, Claude 5-hour usage bar.
- **Glass** — Win11 acrylic backdrop, layered transparency, hand-drawn SVG icons for system apps.

## Install

Two options on the [Releases page](https://github.com/pikammmmm/glassbar/releases/latest):

**Installer (recommended)** — `glassbar_<version>_x64_en-US.msi`
1. Download and run the MSI.
2. Installs to `Program Files\glassbar\`, adds a Start-menu entry, registers an Apps & Features uninstaller.
3. Launch from Start. The dock auto-shows when your cursor reaches the bottom of the screen.
4. (Optional) Open the HUD → **Settings** → toggle *Launch at sign-in* for autostart.

**Portable** — `glassbar.exe`
1. Download the .exe and put it anywhere you like (e.g. `C:\Tools\glassbar.exe`).
2. Double-click to run — no install, no admin prompt, no Start-menu entry.
3. To autostart, drop a shortcut into `shell:startup` (paste that into Run / File Explorer) or toggle *Launch at sign-in* in the HUD's Settings.

## Uninstall

Pick whichever is more convenient:

- **MSI build** — Settings → Apps → Installed apps → glassbar → Uninstall.
  - Or run `uninstall.exe` from `Program Files\glassbar\` (it ships next to `glassbar.exe`). It hands off to `msiexec /x`, restores the Windows taskbar, removes autostart, and wipes user data + cache.
- **Portable** — download `uninstall.exe` from the [Releases page](https://github.com/pikammmmm/glassbar/releases/latest) (or run the one already next to your portable `glassbar.exe`). It runs the same cleanup steps, then you delete `glassbar.exe` yourself.

Either path also restores the standard Windows taskbar in case glassbar was killed without its exit handler running — that's the part you can't get back via "delete the file" alone.

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
