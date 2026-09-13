# Tokpaek

**English** | [Русский](README.md)

A circular quota widget for Claude, Codex, and Antigravity — shows your remaining limit and reset countdown right on top of the AI tool window you're currently working in.

| Continuous Arc (5 hours) | Continuous Arc (7 days) | Segmented Scale (Pool) | Quota Warning Alert |
| :---: | :---: | :---: | :---: |
| <img src="assets/eng/widget_arc_5h.png" width="180" alt="Continuous Arc — 5 Hours" /> | <img src="assets/eng/widget_arc_7d.png" width="180" alt="Continuous Arc — 7 Days" /> | <img src="assets/eng/widget_cells_pool.png" width="180" alt="Segmented Scale — Claude / GPT" /> | <img src="assets/eng/widget_cells_warning.png" width="180" alt="Quota Warning Alert" /> |
| *5h session reset* | *7-day pool reset* | *Shared Claude / GPT pool* | *Exhausted limit indicator* |

The widget automatically follows the currently active tool window and can hide itself when watched apps are in the background ("Smart Focus").

Supported tool families:

- **Claude** — Claude Desktop app and Claude Code / CLI;
- **Codex** — Codex / ChatGPT app and Codex CLI;
- **Antigravity** — Antigravity 2.0, Antigravity IDE and CLI.

## Features

- **Circular dual-arc gauge**: top arc — rolling window (5 hours), bottom arc — seven days or Claude / GPT pool (switchable).
- **Reset timer** in the central pill: toggle between 5 hours and 7 days with a single click; shows exact reset time and countdown.
- **Color palettes**: Gradient (smooth green → yellow → red), Traffic Light, Cyan, Monochrome.
- **Scale format**: Decimal (10 cells), Hour (12 cells), or Continuous smooth arc. Crisp rendering at any scale.
- **Smart Focus**: Widget is visible only when Antigravity, Claude, or ChatGPT is active, staying anchored over that app's window.
- **Always on Top**: Optional toggle if you prefer the widget to remain pinned above all windows.
- **Mouse resize**: Drag the outer ring edge to resize just like a normal window; drag center to move. Size and position are saved.
- **Opacity and polling interval**: Sliders with quick preset buttons.
- Clean and lightweight: No taskbar clutter, left-click to drag, right-click for settings/context menu or via system tray.
- Update check via GitHub release tags — only on user demand.

## Privacy

- **Zero telemetry.** No analytics, tracking pixels, or stats collection.
- **Zero ads.** No banners, affiliate links, or promotional popups.
- **Zero third-party servers.** The app connects only to the official service APIs (Anthropic, OpenAI) and to the local Antigravity language server on 127.0.0.1.
- Tokens are read locally from the already-installed applications' storage and sent solely to the respective provider. Tokpaek never writes to external credential stores.
- The only outgoing network request is checking GitHub for new releases (on demand).
- Settings are saved locally in `%APPDATA%\Tokpaek\settings.json`.

## Data Sources

| Tool | Source |
|---|---|
| Claude | Local Claude Desktop token → official account usage endpoint |
| Codex | `~/.codex/auth.json` (shared between app & CLI) → Codex usage endpoint |
| Antigravity | Local Antigravity language server — same request used by the IDE usage panel |

If an instrument is closed or not installed, its source is marked as unavailable while the others continue working seamlessly.

## Installation

Download `Tokpaek-Setup.exe` from the [Releases](https://github.com/Ilardar/Tokpaek/releases) page and run it. Installs into the user profile without requiring administrator privileges.

You can enable autostart on system boot during installation or toggle it later in settings or the tray menu.

## Usage

- **LMB on circle** — drag to move; **LMB on central pill** — switch countdown timer (FIVE HOURS / SEVEN DAYS).
- **Circle edge** — resize with mouse cursor.
- **RMB on widget** — context menu: Refresh, Home (top-left corner), Always on Top, Settings, Exit.
- **System tray icon** — access the same options and settings window.

## Requirements

- Windows 10/11, x64.
- Installed AI tool: Claude Desktop, Codex CLI/app, or Antigravity.
- For Antigravity — running app or IDE (the quota is queried from its local language server).

## Building from Source

```bash
cargo build --release
```

Installer (requires [Inno Setup 6](https://jrsoftware.org/isdl.php)):

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-installer.ps1
```

---

## Based on [Quotty](https://github.com/confeden/Quotty) — thanks to @confeden
