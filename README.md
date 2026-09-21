<p align="center">
  <img src=".github/assets/okena-icon.png" width="256" alt="Okena">
</p>

# Okena

A fast, native terminal multiplexer built in Rust with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the UI framework from Zed editor).
Tabs, splits, detachable windows, command palette, and automatic workspace restore.

## Installation

### macOS

```bash
curl -fsSL https://raw.githubusercontent.com/n1rna/okena/main/install.sh | bash
```

### Linux

```bash
curl -fsSL https://raw.githubusercontent.com/n1rna/okena/main/install.sh | bash
```

### Windows

```powershell
irm https://raw.githubusercontent.com/n1rna/okena/main/install.ps1 | iex
```

The install script includes built-in auto-update support. On macOS and Linux, Okena is installed to `~/.local/bin/okena`. On Windows, it installs to `%LOCALAPPDATA%\Programs\Okena` with a Start Menu shortcut.

### Coming from upstream okena

This fork publishes its own releases from [`n1rna/okena`](https://github.com/n1rna/okena/releases), versioned independently of [`contember/okena`](https://github.com/contember/okena) and starting at `0.1.0`. An install of upstream okena reports a higher version (such as `0.31.0` or `0.32.0`), so its updater will never see this fork's `0.1.0` as newer. Reinstall once with the install command above; from then on the built-in updater follows this fork's releases.

## Features

### Layout & Window Management
- **Split panes** - Horizontal and vertical splits with drag-to-resize dividers
- **Tabs** - Organize terminals in tabbed containers with reordering support
- **Detachable windows** - Pop out any terminal into a separate floating window and reattach later
- **Fullscreen mode** - Focus on a single terminal with next/previous cycling
- **Minimize/restore** - Collapse terminals to their header to save space
- **Per-terminal zoom** - Adjust zoom level (0.5x to 3.0x) independently per terminal
- **Directional focus navigation** - Move focus between panes using arrow-key shortcuts

### Multi-Project Workspace
- **Project columns** - Manage multiple projects side-by-side with resizable columns
- **Sidebar** - Collapsible project list with tree view of terminals, drag-and-drop reordering, and auto-hide mode
- **Folder colors** - Color-code projects (red, orange, yellow, green, blue, purple, pink)
- **Project switcher** - Quick searchable project navigation overlay
- **Workspace persistence** - Auto-saves full layout, terminal state, and settings to disk

### Terminal
- **Full terminal emulation** - Powered by alacritty_terminal with complete ANSI support
- **Search** - Inline text search with regex support, case sensitivity toggle, and match count
- **Link detection** - Clickable URLs and file paths (supports `file:line:col` syntax)
- **File opener integration** - Open detected files in your editor (VS Code, Cursor, Zed, Sublime, vim, etc.)
- **Bracketed paste mode** - Proper multi-line paste handling with escape sequence injection protection
- **Image paste** - Paste clipboard images (PrintScreen, Snipping Tool, browser "Copy image") into TUIs like Claude Code. On Windows in a WSL terminal this needs `wl-clipboard` installed inside the distro (`sudo apt install wl-clipboard`) so the image is forwarded to WSL's clipboard and attached as `[Image #N]`; without it, the bracketed paste falls back to a `/tmp/...` file-path reference
- **Shift+Enter** - Send literal newline for multi-line input (useful for Claude Code, Python, etc.)
- **Configurable scrollback** - 100 to 100,000 lines
- **Cursor blink** - Toggleable cursor blinking
- **Bell notification** - Visual indicator when a terminal rings the bell
- **Per-terminal shell selection** - Choose a different shell for each terminal
- **Context menu** - Right-click for copy, paste, select all, and link actions

### Session Persistence
- **Session backends** - Keep terminals alive across app restarts using dtach, tmux, or screen (Unix)
- **dtach support** - Lightweight session persistence (preferred backend)
- **Auto-detection** - Automatically selects the best available backend (dtach > tmux > screen)
- **WSL session support** - Session backends work with WSL terminals on Windows
- **Session manager** - Save, load, rename, and delete named workspace sessions
- **Export/import** - Export workspaces to JSON and import them back

### Git Integration
- **Git worktree support** - Create and manage git worktrees as projects directly from the UI
- **Worktree sync watcher** - Auto-discovers new git worktrees every 30 seconds
- **Worktree auto-cleanup** - Removes stale worktree projects when paths no longer exist
- **Worktree path templates** - Configure worktree paths with `{repo}` and `{branch}` variables
- **Merge/stash on close** - Options to merge, stash, fetch, push, or delete branch when closing a worktree
- **Branch detection** - Displays current branch, handles detached HEAD
- **Diff stats** - Tracks lines added/removed with cached git status

### Themes & Appearance
- **Built-in themes** - Dark, Light, Pastel Dark, and High Contrast
- **Auto theme** - Follows system light/dark appearance
- **Custom themes** - Load your own theme from a custom themes directory
- **Configurable fonts** - Font family, size (8-48pt), line height (1.0-3.0), and separate UI font size

### Command Palette & Overlays
- **Command palette** - Searchable list of all actions with keybinding hints
- **File search** - Fast file lookup within a project (respects .gitignore-style filtering)
- **Settings panel** - GUI for all preferences (theme, font, terminal, hooks, per-project settings)
- **Theme selector** - Live-preview theme picker
- **Keybindings help** - Categorized shortcut reference with search
- **File viewer** - Syntax-highlighted file preview with line numbers and search, plus PDF previews with page navigation and zoom
- **Diff viewer** - Unified and side-by-side diff views with syntax highlighting

### Customization
- **Custom keybindings** - Override any shortcut via `keybindings.json`
- **Lifecycle hooks** - Run commands on project open/close and worktree create/close (global or per-project)
- **Per-project settings** - Override global settings per project
- **Shell configuration** - Set default shell or pick per terminal (bash, zsh, fish, cmd, PowerShell, WSL)

### Hook Terminals
- **Hook terminals** - Commands prefixed with `terminal:` in hooks spawn visible PTY terminals (e.g., `terminal: claude -p "fix rebase conflict"`)
- **Hook monitor** - Tracks execution history, status (Running/Succeeded/Failed), and duration for all hooks
- **Git hooks** - `pre_merge`, `post_merge`, `before_worktree_remove`, `worktree_removed`, `on_rebase_conflict`, `on_dirty_worktree_close`
- **Environment variables** - Hooks receive `OKENA_PROJECT_ID`, `OKENA_PROJECT_NAME`, `OKENA_PROJECT_PATH`, `OKENA_BRANCH`, `OKENA_TARGET_BRANCH`, etc.

### Services
- **Project services** - Define services in `okena.yaml` with name, command, cwd, env vars
- **Docker Compose integration** - Auto-detects and manages Docker Compose services
- **Auto-start & restart** - Services can auto-start on project open and auto-restart on crash
- **Service panel** - Monitor service status (Stopped, Starting, Running, Crashed) and ports

### AI Tool Integration
- **Claude Code status** - Real-time service status from status.claude.com
- **Claude Code usage** - OAuth-based usage tracking (5-hour, 7-day rate limits, credits)
- **Codex status & usage** - OpenAI Codex status monitoring with OAuth token refresh
- Both integrations are opt-in via settings toggles

### Remote Control & Companion Apps
- **Remote API** - Local HTTP/WebSocket server for remote terminal control (see `docs/reference/remote.md`)
- **Mobile app** - React Native companion app for Android/iOS over the Rust core via uniffi (see `docs/reference/mobile.md`, code in `mobile/rn`)
- **Web client** - Browser-based terminal access via built-in web UI
- **Secure pairing** - HMAC-SHA256 token auth with rate-limited pairing codes

### Auto-Update
- **Built-in updater** - Background update checks via GitHub Releases
- **Version rollback** - Reinstall an older stable release from Settings or `okena update revert`
- **Config checkpoints** - Restore the matching pre-upgrade config by default, with an explicit keep-current opt-out
- **SHA256 verification** - Downloaded updates are cryptographically verified
- **Homebrew-aware** - Skips self-update when installed via Homebrew

### Platform Support
- **macOS** - Native traffic light buttons, extended PATH for homebrew shells
- **Linux** - Wayland maximize workaround, auto-detected shells
- **Windows** - Custom titlebar, cmd/PowerShell/WSL support with distro detection

### Status Bar
- CPU usage, memory usage, and current time displayed at the bottom

## Building from source

Okena builds on macOS, Linux and Windows. The Rust toolchain is pinned in
`rust-toolchain.toml`, so rustup installs the right version (1.95.0) on the
first `cargo` command — you only install the platform toolchain and Bun by hand.

The build has **two stages that must run in order**: the web client, then Rust.
Skipping the first one is the most common way for a fresh clone to fail — see
[step 2](#2-build-the-web-client-first).

### 1. Prerequisites

Every platform needs:

- **[rustup](https://rustup.rs)** — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **[Bun](https://bun.sh)** — `curl -fsSL https://bun.sh/install | bash` (Windows: `irm bun.sh/install.ps1 | iex`)
- **Git**

Then the platform-specific part:

<details open>
<summary><b>macOS</b></summary>

You need **full Xcode**, not just the Command Line Tools. GPUI compiles its
Metal shaders during the build (`xcrun -sdk macosx metal`), and the Metal
compiler is not part of the CLT — with only the CLT installed the build fails in
`gpui_macos` with `metal shader compilation failed`.

```bash
# Install Xcode from the App Store, then point the toolchain at it:
sudo xcode-select --switch /Applications/Xcode.app

# Xcode 16 and newer ship the Metal compiler as a separate download:
xcodebuild -downloadComponent MetalToolchain
```

Nothing else is required — no Homebrew packages, and no cmake.

</details>

<details open>
<summary><b>Linux (Debian/Ubuntu)</b></summary>

```bash
sudo apt-get update && sudo apt-get install -y \
  build-essential clang libclang-dev pkg-config cmake \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libvulkan-dev libegl1-mesa-dev \
  libssl-dev libfontconfig-dev
```

CI installs this same list without `build-essential`, `clang` and
`libclang-dev`, because GitHub's runner image already has them. On a bare
machine or container you do need them: GPUI generates bindings with bindgen,
which requires libclang.

On other distributions, install the equivalents of the above — the XCB,
xkbcommon, Wayland, Vulkan, EGL, OpenSSL and fontconfig development headers.

</details>

<details open>
<summary><b>Windows</b></summary>

- **Visual Studio 2022** with the **Desktop development with C++** workload,
  which provides the MSVC `x86_64-pc-windows-msvc` toolchain.
- Build from the **x64 Native Tools Command Prompt for VS 2022**. In a plain
  shell, Git for Windows' `link.exe` shadows the MSVC linker on `PATH` and the
  link step fails.

</details>

### 2. Build the web client first

This step is **not optional**, even if you never open the web UI.
`okena-remote-server` embeds `web/dist` into the binary at compile time with
`RustEmbed`, and `web/dist` is not checked in. Build it and `cargo build` stops
with:

```
#[derive(RustEmbed)] folder '.../web/dist' does not exist
```

```bash
cd web
bun install --frozen-lockfile
bun run build
cd ..
```

You only need to repeat this when the web client itself changes.

### 3. Build

```bash
cargo build --release -p okena -p okena-daemon
```

Build **both** binaries. `okena` is the desktop app and `okena-daemon` is the
GPUI-free daemon that owns all the real state (see
[ADR-0001](docs/decisions/0001-headless-two-process-daemon.md)). The app looks
for an `okena-daemon` sibling next to itself and quietly falls back to
`okena --headless` when it is missing, so a build without it works but is not
what ships.

They land in `target/release/`.

### 4. Run

```bash
cargo run
```

That builds and runs a debug binary, which is slower but compiles faster — the
usual choice while developing. For the release build, run `target/release/okena`
directly.

### macOS: build a .app bundle

```bash
./scripts/bundle-macos.sh           # builds, then writes dist/Okena.app
./scripts/bundle-macos.sh --dmg     # also writes dist/Okena-<version>-<target>.dmg
```

Useful flags: `--target <triple>` to pick the architecture
(`aarch64-apple-darwin` or `x86_64-apple-darwin`) and `--skip-build` to bundle
binaries you have already built. The script reads the version from
`Cargo.toml`.

### Running the tests

```bash
cargo test --workspace
```

Pass `--workspace`. The workspace declares no `default-members`, so a bare
`cargo test` runs the root `okena` package alone and silently skips every crate
that holds the actual tests.

Some tests skip themselves when a prerequisite is missing, so they pass without
proving anything. To run the full set:

```bash
sudo apt-get install -y dtach zsh fish   # session-teardown and shell-quoting tests
rustup target add wasm32-wasip2          # the extension host builds WASM fixtures
cargo build -p okena-tui                 # the end-to-end tests drive this binary
```

Then set `OKENA_REQUIRE_SHELLS=1`, `OKENA_REQUIRE_WASM=1` and
`OKENA_REQUIRE_TUI=1`, which turn each of those skips into a failure. That is
what CI does, so that a green run proves what it claims.

See [docs/reference/testing.md](docs/reference/testing.md) for test selection
rules and the GPUI test harness.

## Keyboard Shortcuts

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| New terminal | Cmd+T | Ctrl+T |
| Close terminal | Cmd+W | Ctrl+W |
| Split horizontal | Cmd+D | Ctrl+D |
| Split vertical | Cmd+Shift+D | Ctrl+Shift+D |
| Navigate panes | Cmd+Alt+Arrow | Ctrl+Alt+Arrow |
| Next/prev terminal | Cmd+Shift+]/[ | Ctrl+Tab / Ctrl+Shift+Tab |
| Fullscreen terminal | Shift+Escape | Shift+Escape |
| Command palette | Cmd+Shift+P | Ctrl+Shift+P |
| File search | Cmd+P | Ctrl+P |
| Find | Cmd+F | Ctrl+F |
| Copy | Cmd+C | Ctrl+C |
| Paste | Cmd+V | Ctrl+V |
| Zoom in/out | Cmd++/- | Ctrl++/- |
| Reset zoom | Cmd+0 | Ctrl+0 |
| Toggle sidebar | Cmd+B | Ctrl+B |
| Settings | Cmd+, | Ctrl+, |

All shortcuts are customizable via `keybindings.json` in your platform's config directory.

## Configuration

Settings are stored in the platform's config directory (macOS: `~/Library/Application Support/okena/`, Linux/Windows: `~/.config/okena/` / `%APPDATA%\okena\`):

| File | Purpose |
|------|---------|
| `settings.json` | Theme, font, shell, scrollback, hooks, and other preferences |
| `workspace.json` | Projects, layouts, and terminal state |
| `keybindings.json` | Custom keyboard shortcuts |
| `themes/*.json` | Custom theme files |
| `okena.yaml` (project root) | Project services and Docker Compose configuration |

## Documentation

| Guide | Description |
|-------|-------------|
| [Configuration](docs/reference/configuration.md) | Settings, keybindings, custom themes, per-project overrides |
| [Lifecycle Hooks](docs/reference/hooks.md) | Hook terminals, git hooks, environment variables |
| [Project Services](docs/reference/services.md) | okena.yaml, Docker Compose integration, auto-restart |
| [Git Worktrees](docs/reference/worktrees.md) | Worktree management, sync watcher, path templates |
| [Remote Control API](docs/reference/remote.md) | HTTP/WebSocket API, pairing, authentication |
| [Mobile Client](docs/reference/mobile.md) | React Native (uniffi) mobile companion app |

## Dependencies

- **GPUI** + **gpui-component** - UI framework
- **alacritty_terminal** - Terminal emulation
- **portable-pty** - PTY management
- **smol** - Async runtime
- **tokio** + **axum** - Remote control server
- **syntect** - Syntax highlighting
- **serde_yaml** - Service config parsing

## A Note on Authorship

> **This codebase has not been contaminated by human hands.**
>
> Every line of code, every architectural decision, every meticulously placed semicolon — pure, unfiltered Claude Opus and OpenAI Codex, each confidently taking credit for the good parts.
> The human's contribution was limited to typing vague requirements like "make it work" and then pressing `Enter` to approve tool calls with the mass-produced enthusiasm of a factory worker.
>
> If you find a bug, rest assured — it's not a bug. It's the AI testing whether you're paying attention.
>
> *Humans are kindly thanked for providing electricity.*

## License

MIT
