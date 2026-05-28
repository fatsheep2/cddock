# cddock Product Plan

## Product Shape

`cddock` should be a TUI-first CDDA companion, with installer helpers around it.

The clean split is:

- install script: bootstrap dependencies and install the `cddock` binary
- TUI: configure CDDA, install/update builds, launch game, add to Steam
- helper commands: expose the same operations for scripting

This keeps the risky root-level package installation outside the everyday TUI,
while still letting the TUI show system status and guide the user.

The current prototype is already a Rust `ratatui`/`crossterm` TUI. It has the
screen structure and input model, but the actions still need to be connected to
real CDDA download, install, launch, and Steam shortcut code.

It also has the first language and config layer:

- first launch detects system language from `LC_ALL`, `LC_MESSAGES`, or `LANG`
- Chinese is selected for `zh*` locales
- English is the fallback
- Settings can show and switch the current language
- explicit language choice is saved to `~/.config/cddock/config.toml`
- config also stores CDDA path, release channel, Steam shortcut name, and whether
  to use a Konsole-backed Steam Deck game-mode entry

Most functional actions are still placeholders. The next implementation layer
should replace TODO actions with real platform detection, dependency checks,
CDDA release download/install, save backup, Steam shortcut writing, and launch
commands.

## Core TUI Pages

### Home

- detected platform
- configured game library path
- installed `cddock` version
- graphics dependency status
- Steam shortcut status
- primary actions: Launch, Install Game, Check Dependencies, Settings

### Versions

- show local project root, defaulting to `~/.local/cddock`
- store game binaries under `versions/<release-tag>` (Catapult `current/`, catman `builds/`)
- store shared user data under one `userdata/` directory across installed builds (Catapult/catman userdata model)
- keep one shared userdata directory and rely on backups before risky channel/build switches
- support multiple installed versions side by side
- select/switch active build
- manage saves/config backup
- show currently active build

### Install

The install flow should be a dedicated page opened from Versions -> Install
Game.

Flow:

1. choose Stable or Experimental
2. fetch the available download list
3. show a selectable build list
4. download the selected build
5. extract it into the local game library
6. optionally switch active build to the new install

This mirrors the useful Catapult/catman idea: keep downloaded versions under a
launcher-owned directory instead of overwriting one global game folder.

### Native Graphics Dependencies

This page should merge dependency checking and read-only risk handling into one
repair workflow.

The normal path should be:

1. launch/check CDDA
2. detect SDL-related failure or missing library
3. ask whether to run dependency repair
4. execute the platform-specific repair plan

Steam Deck / SteamOS dependency list:

- `sdl2`
- `sdl2_image`
- `sdl2_mixer`
- `sdl2_ttf`

SteamOS-specific flow:

1. explain the detected missing SDL dependency and read-only requirement
2. ask for confirmation
3. require sudo/root
4. run `steamos-readonly disable`
5. refresh keyrings
6. install SDL2 packages
7. run `steamos-readonly enable`

Arch flow:

1. ask for confirmation
2. require sudo/root
3. refresh keyrings
4. install SDL2 packages

macOS flow:

1. detect Homebrew
2. if missing, explain that Homebrew is required
3. install `sdl2`, `sdl2_image`, `sdl2_mixer`, `sdl2_ttf`

Windows flow:

1. prefer bundled release assets when possible
2. avoid asking the user to use Windows translation on Steam Deck
3. use native Windows packages only for Windows users

### Steam Integration

This should be a dedicated page, not buried inside installation.

Actions:

- add `cddock` as a Steam game-mode TUI entry through Konsole
- add CDDA as a non-Steam game
- choose launch target
- choose icon/banner later
- optionally rename shortcut to `Cataclysm: Dark Days Ahead`

Default recommendation:

- offer `Cataclysm: Dark Days Ahead` as the shortcut name because community
  controller layouts are more likely to match that title
- also allow a short name like `CDDA`

### Steam Deck Game Mode TUI

Steam Deck can run the TUI through its built-in Konsole app. This is the first
implementation path because it keeps the app as a normal `ratatui`/`crossterm`
program while still making it usable in game mode.

The Steam shortcut for the management TUI should launch Konsole and run:

```sh
cddock
```

The exact command may need Deck-side validation, but the desired behavior is:

- open directly in Steam game mode
- no desktop workflow required after installation
- use a large readable font
- controller mapping should cover arrows, enter, escape, tab, and quick actions
- support `hjkl` because CDDA players often already use those movement keys
- keep CDDA itself as a separate Steam shortcut for community controller layouts

This gives users two game-mode entries:

```text
CDDock
Cataclysm: Dark Days Ahead
```

`CDDock` is for management, dependency status, version selection, save/load
helpers, and launching. `Cataclysm: Dark Days Ahead` is for direct gameplay.

Recommended Steam Input mapping for `CDDock`:

```text
D-pad / left stick -> arrow keys
A -> Enter
B -> Esc
Menu -> q
L1/R1 -> Tab or h/l for focus switching
```

The app should treat both input families as equivalent:

```text
h == Left / focus previous panel
j == Down / next item
k == Up / previous item
l == Right / focus next panel
Tab == switch focus
```

### Settings

- language: view current language and switch Chinese / English
- CDDA install path
- save/config path
- release channel
- terminal/theme settings
- Steam integration settings

## Installer Flow

The install script should remain intentionally narrow:

1. choose language
2. detect device/platform
3. explain dependency and system-readonly risks
4. ask for confirmation
5. require root when package manager changes are needed
6. install platform dependencies
7. install `cddock` to the user-local bin directory
8. restore SteamOS read-only mode if it was disabled
9. ask whether to add CDDA to Steam
10. ask whether to name it `Cataclysm: Dark Days Ahead`

## Install Locations

Steam Deck default:

```text
/home/deck/.local/bin/cddock
```

Generic Linux default:

```text
$HOME/.local/bin/cddock
```

macOS default:

```text
/usr/local/bin/cddock
```

Windows default:

```text
%LOCALAPPDATA%\\cddock\\cddock.exe
```

## Implementation Recommendation

Use Rust for the app:

- `ratatui` for the interface
- `crossterm` for terminal input/rendering
- `steamlocate` or local Steam config parsing for shortcut management
- `serde`/`toml` for config
- separate privileged install script from unprivileged TUI logic

Avoid making the TUI responsible for package-manager mutation directly. It can
detect missing dependencies and call the installer only after explicit user
confirmation.
