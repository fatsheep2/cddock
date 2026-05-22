# cddock

`cddock` is a cross-platform TUI companion for Cataclysm: Dark Days Ahead.

The first goal is not to replace CDDA itself. It should make the recurring setup
and launch work simple:

- install native graphics dependencies when the platform supports it
- manage CDDA builds and local data directories
- launch graphical CDDA without Windows translation layers on Steam Deck
- run the TUI in Steam Deck game mode through the built-in Konsole app
- add CDDA to Steam as a non-Steam game with a controller-layout friendly name
- keep the same basic workflow available on Windows, macOS, Linux, Arch, and SteamOS

## Name

The project directory is named `cddock`.

Reasons:

- short enough to type
- easier to remember than `cdda-tui`
- not Steam Deck specific
- leaves room for both TUI and install/helper commands
- similar spirit to `catman`, but more focused on control/setup workflows

## Suggested Structure

```text
cddock/
  README.md
  Cargo.toml
  src/
    main.rs
  docs/
    product-plan.md
  scripts/
    install.sh
```

The first implementation uses Rust:

- `ratatui`/`crossterm` for the TUI
- one native binary for Windows/macOS/Linux
- good Steam Deck/Arch compatibility
- straightforward packaging through GitHub Releases

Current prototype:

- real terminal TUI
- restrained TUI styling with ASCII-like page badges and action tags, not emoji-heavy UI
- top status bar for language, game library, release channel, and active build
- system-language detection on first launch through `LC_ALL`, `LC_MESSAGES`, or `LANG`
- Chinese and English UI text
- Settings page action to view and switch the current language
- config file persistence for language and common settings
- focus-based navigation for page list and page actions
- vim-style movement: `j/k` for vertical movement, `h/l` for left/right focus movement
- arrow-key navigation
- `Tab` to switch focus between page list and actions
- `Enter` to enter a page or activate an action
- `Esc` to return Home
- `q` or `Ctrl-C` to quit
- Steam Deck friendly because Steam Input can map D-pad/stick to arrows or
  `h/j/k/l`

Visual direction:

- page badges: `[H]`, `[V]`, `[+]`, `[!]`, `[S]`, `[*]`, `[?]`
- action tags: `[RUN]`, `[GET]`, `[FIX]`, `[STM]`, `[SET]`
- status chips: `[LANG:...]`, `[ROOT:...]`, `[CH:...]`, `[ACTIVE:...]`
- avoid emoji dependency so Konsole and Steam Deck fonts stay predictable

Run locally:

```sh
cargo run
```

Suggested future source layout after the prototype grows:

```text
cddock/
  Cargo.toml
  crates/
    cddock-cli/
    cddock-core/
    cddock-tui/
  scripts/
    install.sh
    add-to-steam.sh
```

## Steam Deck Game Mode

The first Steam Deck path should use the built-in Konsole app as the terminal
host for the TUI.

That means `cddock` can stay a real `ratatui` application instead of becoming a
custom SDL/winit renderer. The installer should add a Steam shortcut that starts
Konsole in fullscreen/game-mode-friendly form and runs:

```sh
cddock
```

The game itself should still be added separately as:

```text
Cataclysm: Dark Days Ahead
```

That keeps community controller layout matching focused on the game shortcut,
while `cddock` remains the management and save/load companion.

Suggested Steam Input mapping for the `CDDock` shortcut:

```text
D-pad / left stick: arrow keys
A: Enter
B: Esc
Menu: q
L1/R1: Tab or h/l for focus switching
```

For users who already use `hjkl` heavily in CDDA, the same movement muscle
memory works inside `cddock`.

## Configuration

`cddock` stores common settings in:

```text
~/.config/cddock/config.toml
```

On Windows, the intended path is:

```text
%APPDATA%\cddock\config.toml
```

Current saved settings:

- `language`: `system`, `english`, or `chinese`
- `cdda_path`: default CDDA install path
- `game_root`: CDDock project root, defaulting to `~/.local/cddock`
- `active_build`: selected build under the game library
- `release_channel`: default release channel, currently `experimental`
- `steam_shortcut_name`: default gameplay shortcut name
- `use_steam_deck_konsole`: whether the TUI should use a Konsole-backed Steam entry

Language defaults to the system language until the user switches it in Settings.
After that, the explicit choice is saved and takes priority on future launches.

## Current Limitations

Implemented in the TUI:

- scan installed builds under the game library
- select and persist the active build
- fetch stable/experimental GitHub releases and download platform assets
- extract builds into `~/.local/cddock/versions/<build-tag>`
- keep shared user data in `userdata-stable/` and `userdata-experimental/` (Catapult/catman model)
- launch with `--userdir` and optional `--world` (catman-style)
- zip backups of the active channel's `save/` directory
- GitHub asset matching and stable-tag discovery inspired by catman

Still placeholders:

- SDL2 dependency detection and repair from the TUI
- Steam shortcut writing
- settings path editing inside the TUI

The intended layout is:

```text
~/.local/cddock/
  versions/
    cdda-experimental-2026-05-22-1007/   # game binaries only
  userdata-experimental/
    save/ gfx/ mods/ sound/ font/ config/ ...
  userdata-stable/
    save/ gfx/ mods/ sound/ font/ config/ ...
  downloads/            # temporary archives (catman-style)
  backups/              # zip save backups
```

Set `GITHUB_TOKEN` if GitHub rate-limits release downloads (catman recommendation).

The Versions page manages installed builds. The Install page should choose
stable or experimental, fetch available downloads, and let the user select which
build to download.

## Install Script

The bootstrap script lives at:

```text
scripts/install.sh
```

After publishing a GitHub release, install the latest build with:

```sh
curl -fsSL https://raw.githubusercontent.com/fatsheep2/cddock/main/scripts/install.sh | bash
```

For a specific release tag:

```sh
curl -fsSL https://raw.githubusercontent.com/fatsheep2/cddock/main/scripts/install.sh | CDDOCK_VERSION=v0.1.0 bash
```

The installer downloads the matching GitHub Release asset for the current
platform and installs it to `~/.local/bin/cddock` (or `/home/deck/.local/bin`
on Steam Deck).

On Steam Deck, the installer also tries to add a `CDDock` non-Steam game
shortcut that starts Konsole and runs:

```sh
/home/deck/.local/bin/cddock
```

Set `CDDOCK_ADD_STEAM=0` to skip shortcut creation. Steam must be fully
restarted before the new shortcut appears in Game Mode.

On SteamOS/Arch, it can also install the native CDDA tiles dependencies:

```text
sdl2 sdl2_image sdl2_mixer sdl2_ttf
```

SteamOS repair disables read-only mode, refreshes keyrings, installs SDL2
packages, then re-enables read-only mode. Arch refreshes keyrings and installs
the same packages directly. Set `CDDOCK_INSTALL_DEPS=0` to skip dependency
installation.

Release packages are built by GitHub Actions:

- `cddock-latest-x86_64-unknown-linux-gnu.tar.gz` for Steam Deck/Linux
- `cddock-latest-aarch64-apple-darwin.tar.gz` for Apple Silicon macOS
- `cddock-latest-x86_64-apple-darwin.tar.gz` for Intel macOS
