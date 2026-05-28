#!/usr/bin/env bash
set -euo pipefail

APP_NAME="cddock"
REPO="fatsheep2/cddock"
STEAM_SHORTCUT_RECOMMENDED_NAME="Cataclysm: Dark Days Ahead"
SDL_PACKAGES_ARCH=(sdl2 sdl2_image sdl2_mixer sdl2_ttf freetype2 zip)

LANG_CHOICE="${CDDOCK_LANG:-auto}"
IS_STEAMOS=0
IS_ARCH=0
READONLY_DISABLED=0
INSTALL_DEPS="${CDDOCK_INSTALL_DEPS:-1}"
INSTALLED_BINARY_PATH=""

msg() {
  case "$LANG_CHOICE" in
    zh) printf '%s\n' "$1" ;;
    *) printf '%s\n' "$2" ;;
  esac
}

ask_yes_no() {
  local prompt_zh="$1"
  local prompt_en="$2"
  local default="${3:-no}"
  local answer

  if [[ ! -t 0 ]]; then
    [[ "$default" == "yes" ]]
    return
  fi

  case "$LANG_CHOICE" in
    zh) read -r -p "$prompt_zh [y/N] " answer ;;
    *) read -r -p "$prompt_en [y/N] " answer ;;
  esac

  case "$answer" in
    y|Y|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

choose_language() {
  if [[ "$LANG_CHOICE" == "auto" ]]; then
    case "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" in
      zh*) LANG_CHOICE="zh" ;;
      *) LANG_CHOICE="en" ;;
    esac
  fi

  if [[ -t 0 ]]; then
    printf '1. 选择语言 / Choose language\n'
    printf '   [1] 中文\n'
    printf '   [2] English\n'
    read -r -p '> ' choice
    case "$choice" in
      1) LANG_CHOICE="zh" ;;
      2) LANG_CHOICE="en" ;;
      *) ;;
    esac
  fi
}

detect_platform() {
  if [[ -r /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    case "${ID:-}" in
      steamos|holo) IS_STEAMOS=1 ;;
      arch) IS_ARCH=1 ;;
    esac

    if [[ "${VARIANT_ID:-}" == "steamdeck" ]] || grep -qi 'steamos\|holo' /etc/os-release; then
      IS_STEAMOS=1
    fi
  fi

  if [[ "$(uname -s)" == "Darwin" ]]; then
    msg "检测到 macOS。" "Detected macOS."
  elif [[ "$IS_STEAMOS" -eq 1 ]]; then
    msg "检测到 SteamOS / Steam Deck 环境。" "Detected SteamOS / Steam Deck environment."
  elif [[ "$IS_ARCH" -eq 1 ]]; then
    msg "检测到 Arch Linux。" "Detected Arch Linux."
  else
    msg "当前系统不是 SteamOS/Arch/macOS；将只安装 ${APP_NAME}，不会修改系统包。" \
      "This system is not SteamOS/Arch/macOS; only ${APP_NAME} will be installed and system packages will not be changed."
  fi
}

run_as_root() {
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    msg "安装系统依赖需要 root 权限，但未检测到 sudo。" \
      "Installing system dependencies requires root, but sudo was not found."
    return 1
  fi
}

restore_steamos_readonly() {
  if [[ "$READONLY_DISABLED" -eq 1 ]] && command -v steamos-readonly >/dev/null 2>&1; then
    msg "正在重新开启 SteamOS 系统只读模式..." \
      "Re-enabling SteamOS read-only mode..."
    run_as_root steamos-readonly enable
  fi
}

install_arch_sdl_packages() {
  local missing=()
  local pkg

  if ! command -v pacman >/dev/null 2>&1; then
    msg "未检测到 pacman，跳过 SDL2 依赖检测。" \
      "pacman was not found; skipping SDL2 dependency detection."
    return 0
  fi

  for pkg in "${SDL_PACKAGES_ARCH[@]}"; do
    if ! pacman -Q "$pkg" >/dev/null 2>&1; then
      missing+=("$pkg")
    fi
  done

  if [[ "${#missing[@]}" -eq 0 ]]; then
    msg "SDL2 依赖已安装，跳过系统依赖修复。" \
      "SDL2 dependencies are already installed; skipping system dependency repair."
    return 0
  fi

  msg "缺少依赖：${missing[*]}" \
    "Missing dependencies: ${missing[*]}"

  if [[ "$IS_STEAMOS" -eq 1 ]]; then
    msg "SteamOS 默认系统只读。安装 SDL2 依赖需要临时关闭只读模式，安装完成后会自动重新开启。" \
      "SteamOS uses a read-only system image by default. SDL2 dependency installation needs to temporarily disable read-only mode and will re-enable it afterward."

    if ! ask_yes_no "是否继续安装原生图形版 CDDA 所需依赖？" \
      "Continue installing dependencies required for native graphical CDDA?"; then
      msg "已跳过依赖安装。" "Dependency installation skipped."
      return 0
    fi

    trap restore_steamos_readonly EXIT

    msg "正在关闭 SteamOS 系统只读模式..." \
      "Disabling SteamOS read-only mode..."
    run_as_root steamos-readonly disable
    READONLY_DISABLED=1

    msg "正在刷新 pacman keyring..." \
      "Refreshing pacman keyring..."
    run_as_root pacman-key --init
    run_as_root pacman-key --populate holo archlinux
    run_as_root pacman -Sy holo-keyring archlinux-keyring --overwrite="*"
    run_as_root pacman -Syy
  else
    if ! ask_yes_no "是否安装 CDDA tiles 所需 SDL2 依赖？" \
      "Install SDL2 dependencies required by CDDA tiles?"; then
      msg "已跳过依赖安装。" "Dependency installation skipped."
      return 0
    fi
    run_as_root pacman -Syy
  fi

  run_as_root pacman -S --needed "${missing[@]}"
}

install_macos_sdl_packages() {
  local brew_packages=(sdl2 sdl2_image sdl2_mixer sdl2_ttf)
  local missing=()
  local pkg

  if ! command -v brew >/dev/null 2>&1; then
    msg "未检测到 Homebrew。macOS 版本建议先安装 Homebrew，再安装 SDL2 依赖。" \
      "Homebrew was not found. On macOS, install Homebrew first, then install SDL2 dependencies."
    return 0
  fi

  for pkg in "${brew_packages[@]}"; do
    if ! brew list --formula "$pkg" >/dev/null 2>&1; then
      missing+=("$pkg")
    fi
  done

  if [[ "${#missing[@]}" -eq 0 ]]; then
    msg "SDL2 依赖已安装，跳过 Homebrew 安装。" \
      "SDL2 dependencies are already installed; skipping Homebrew install."
    return 0
  fi

  msg "缺少依赖：${missing[*]}" \
    "Missing dependencies: ${missing[*]}"

  if ask_yes_no "是否通过 Homebrew 安装 CDDA tiles 所需 SDL2 依赖？" \
    "Install SDL2 dependencies required by CDDA tiles through Homebrew?"; then
    brew install "${missing[@]}"
  fi
}

release_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Linux:x86_64) printf 'x86_64-unknown-linux-gnu' ;;
    Darwin:arm64) printf 'aarch64-apple-darwin' ;;
    Darwin:x86_64) printf 'x86_64-apple-darwin' ;;
    *)
      msg "暂不支持当前平台：${os}/${arch}" \
        "Unsupported platform: ${os}/${arch}"
      exit 1
      ;;
  esac
}

install_binary() {
  local target_dir target_path target version url fallback_url tmp archive binary tmp_target
  local script_dir

  if [[ "$IS_STEAMOS" -eq 1 ]] && [[ -d /home/deck ]]; then
    target_dir="/home/deck/.local/bin"
  else
    target_dir="${HOME}/.local/bin"
  fi

  mkdir -p "$target_dir"
  target_path="${target_dir}/${APP_NAME}"
  target="$(release_target)"
  version="${CDDOCK_VERSION:-latest}"
  script_dir=""
  if [[ "${BASH_SOURCE[0]+set}" == "set" ]] && [[ -n "${BASH_SOURCE[0]}" ]]; then
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  fi

  if [[ -n "$script_dir" ]] && [[ -x "${script_dir}/cddock" ]]; then
    msg "正在安装本地安装包中的 ${APP_NAME}。" \
      "Installing ${APP_NAME} from the local package."
    tmp_target="$(mktemp "${target_dir}/.${APP_NAME}.XXXXXX")"
    cp "${script_dir}/cddock" "$tmp_target"
    chmod +x "$tmp_target"
    mv -f "$tmp_target" "$target_path"
    INSTALLED_BINARY_PATH="$target_path"
    msg "已安装 ${APP_NAME} 到：${target_path}" \
      "Installed ${APP_NAME} to: ${target_path}"
    return 0
  fi

  if [[ "$version" == "latest" ]]; then
    url="https://github.com/${REPO}/releases/latest/download/cddock-latest-${target}.tar.gz"
    fallback_url="https://github.com/${REPO}/releases/download/dev-snapshot/cddock-dev-snapshot-${target}.tar.gz"
  elif [[ "$version" == "dev-snapshot" ]]; then
    url="https://github.com/${REPO}/releases/download/dev-snapshot/cddock-dev-snapshot-${target}.tar.gz"
    fallback_url=""
  else
    url="https://github.com/${REPO}/releases/download/${version}/cddock-${version}-${target}.tar.gz"
    fallback_url=""
  fi

  tmp="$(mktemp -d)"
  archive="${tmp}/cddock.tar.gz"
  msg "正在下载 ${APP_NAME}：${url}" \
    "Downloading ${APP_NAME}: ${url}"
  if ! curl -fsSL "$url" -o "$archive"; then
    if [[ -z "$fallback_url" ]]; then
      msg "下载失败。请确认该版本已发布当前平台安装包。" \
        "Download failed. Make sure this version has a release package for the current platform."
      exit 1
    fi
    msg "latest 安装包不可用，改用 dev-snapshot：${fallback_url}" \
      "The latest package was unavailable; trying dev-snapshot: ${fallback_url}"
    curl -fsSL "$fallback_url" -o "$archive"
  fi
  tar -xzf "$archive" -C "$tmp"
  binary="$(find "$tmp" -type f -name cddock | head -n 1)"
  if [[ -z "$binary" ]]; then
    msg "安装包中未找到 cddock 二进制。" \
      "Could not find cddock binary in the package."
    exit 1
  fi
  tmp_target="$(mktemp "${target_dir}/.${APP_NAME}.XXXXXX")"
  cp "$binary" "$tmp_target"
  chmod +x "$tmp_target"
  mv -f "$tmp_target" "$target_path"
  INSTALLED_BINARY_PATH="$target_path"

  msg "已安装 ${APP_NAME} 到：${target_path}" \
    "Installed ${APP_NAME} to: ${target_path}"

  if ! printf '%s' "$PATH" | grep -q "${target_dir}"; then
    msg "提示：如果命令不可用，请把 ${target_dir} 加入 PATH。" \
      "Tip: if the command is unavailable, add ${target_dir} to PATH."
  fi
}

add_steam_shortcut() {
  local target_path="$1"
  local added=0
  local config_dir
  local shortcut_file
  local real_shortcut
  local seen_shortcuts=""

  if ! command -v python3 >/dev/null 2>&1; then
    msg "未检测到 python3，无法自动写入 Steam 快捷方式。" \
      "python3 was not found; cannot write Steam shortcut automatically."
    return 1
  fi

  shopt -s nullglob
  for config_dir in "${HOME}/.local/share/Steam/userdata"/*/config "${HOME}/.steam/steam/userdata"/*/config; do
    [[ -d "$config_dir" ]] || continue
    shortcut_file="${config_dir}/shortcuts.vdf"
    real_shortcut="$(readlink -f "$shortcut_file" 2>/dev/null || printf '%s' "$shortcut_file")"
    case " ${seen_shortcuts} " in
      *" ${real_shortcut} "*) continue ;;
    esac
    seen_shortcuts="${seen_shortcuts} ${real_shortcut}"
    CDDOCK_SHORTCUTS_VDF="$shortcut_file" \
    CDDOCK_BIN="$target_path" \
    python3 <<'PY'
import os
import struct
import zlib
from collections import OrderedDict
from pathlib import Path

path = Path(os.environ["CDDOCK_SHORTCUTS_VDF"])
cddock = os.environ["CDDOCK_BIN"]
home = str(Path.home())
konsole = "/usr/bin/konsole"

def read_cstr(data, pos):
    end = data.index(b"\x00", pos)
    return data[pos:end].decode("utf-8", "replace"), end + 1

def parse_object(data, pos):
    obj = OrderedDict()
    while pos < len(data):
        typ = data[pos]
        pos += 1
        if typ == 8:
            break
        key, pos = read_cstr(data, pos)
        if typ == 0:
            value, pos = parse_object(data, pos)
        elif typ == 1:
            value, pos = read_cstr(data, pos)
        elif typ == 2:
            value = struct.unpack_from("<i", data, pos)[0]
            pos += 4
        else:
            raise ValueError(f"unsupported vdf type {typ}")
        obj[key] = (typ, value)
    return obj, pos

def pack_cstr(text):
    return text.encode("utf-8") + b"\x00"

def dump_object(obj):
    out = bytearray()
    for key, (typ, value) in obj.items():
        out.append(typ)
        out.extend(pack_cstr(key))
        if typ == 0:
            out.extend(dump_object(value))
        elif typ == 1:
            out.extend(pack_cstr(str(value)))
        elif typ == 2:
            out.extend(struct.pack("<i", int(value)))
        else:
            raise ValueError(f"unsupported vdf type {typ}")
    out.append(8)
    return bytes(out)

def empty_root():
    return OrderedDict([("shortcuts", (0, OrderedDict()))])

if path.exists() and path.stat().st_size:
    root, _ = parse_object(path.read_bytes(), 0)
else:
    root = empty_root()

shortcuts = root.setdefault("shortcuts", (0, OrderedDict()))[1]
for key, (_, entry) in list(shortcuts.items()):
    app_name = entry.get("AppName", (1, ""))[1]
    if app_name == "CDDock":
        del shortcuts[key]

appid = (zlib.crc32((konsole + "CDDock").encode("utf-8")) | 0x80000000) & 0xFFFFFFFF
if appid >= 0x80000000:
    signed_appid = appid - 0x100000000
else:
    signed_appid = appid

entry = OrderedDict([
    ("appid", (2, signed_appid)),
    ("AppName", (1, "CDDock")),
    ("Exe", (1, f'"{konsole}"')),
    ("StartDir", (1, f'"{home}"')),
    ("icon", (1, "")),
    ("ShortcutPath", (1, "")),
    ("LaunchOptions", (1, f'--workdir "{home}" --nofork -e "{cddock}"')),
    ("IsHidden", (2, 0)),
    ("AllowDesktopConfig", (2, 1)),
    ("AllowOverlay", (2, 1)),
    ("OpenVR", (2, 0)),
    ("Devkit", (2, 0)),
    ("DevkitGameID", (1, "")),
    ("LastPlayTime", (2, 0)),
    ("tags", (0, OrderedDict([("0", (1, "cddock"))]))),
])

next_index = 0
if shortcuts:
    numeric = [int(key) for key in shortcuts.keys() if key.isdigit()]
    next_index = max(numeric, default=-1) + 1
shortcuts[str(next_index)] = (0, entry)

path.parent.mkdir(parents=True, exist_ok=True)
if path.exists():
    backup = path.with_suffix(".vdf.cddock.bak")
    backup.write_bytes(path.read_bytes())
path.write_bytes(dump_object(root))
PY
    added=1
    msg "已写入 Steam 快捷方式：${shortcut_file}" \
      "Wrote Steam shortcut: ${shortcut_file}"
  done
  shopt -u nullglob

  if [[ "$added" -eq 0 ]]; then
    msg "未找到 Steam userdata/config 目录，无法自动添加快捷方式。" \
      "Steam userdata/config directory was not found; shortcut was not added."
    return 1
  fi

  msg "请完全退出并重新打开 Steam，新的 CDDock 非 Steam 游戏才会出现。" \
    "Fully quit and reopen Steam for the new CDDock non-Steam game to appear."
}

steam_shortcut_current() {
  local target_path="$1"
  local config_dir
  local shortcut_file
  local real_shortcut
  local seen_shortcuts=""

  if ! command -v python3 >/dev/null 2>&1; then
    return 1
  fi

  shopt -s nullglob
  for config_dir in "${HOME}/.local/share/Steam/userdata"/*/config "${HOME}/.steam/steam/userdata"/*/config; do
    [[ -d "$config_dir" ]] || continue
    shortcut_file="${config_dir}/shortcuts.vdf"
    [[ -f "$shortcut_file" ]] || continue
    real_shortcut="$(readlink -f "$shortcut_file" 2>/dev/null || printf '%s' "$shortcut_file")"
    case " ${seen_shortcuts} " in
      *" ${real_shortcut} "*) continue ;;
    esac
    seen_shortcuts="${seen_shortcuts} ${real_shortcut}"
    if CDDOCK_SHORTCUTS_VDF="$shortcut_file" CDDOCK_BIN="$target_path" python3 <<'PY'
import os
import struct
from pathlib import Path

path = Path(os.environ["CDDOCK_SHORTCUTS_VDF"])
cddock = os.environ["CDDOCK_BIN"]
home = str(Path.home())
expected = {
    "AppName": "CDDock",
    "Exe": '"/usr/bin/konsole"',
    "StartDir": f'"{home}"',
    "LaunchOptions": f'--workdir "{home}" --nofork -e "{cddock}"',
}

def read_cstr(data, pos):
    end = data.index(b"\x00", pos)
    return data[pos:end].decode("utf-8", "replace"), end + 1

def parse_object(data, pos):
    obj = {}
    while pos < len(data):
        typ = data[pos]
        pos += 1
        if typ == 8:
            break
        key, pos = read_cstr(data, pos)
        if typ == 0:
            value, pos = parse_object(data, pos)
        elif typ == 1:
            value, pos = read_cstr(data, pos)
        elif typ == 2:
            value = struct.unpack_from("<i", data, pos)[0]
            pos += 4
        else:
            raise ValueError(f"unsupported vdf type {typ}")
        obj[key] = (typ, value)
    return obj, pos

try:
    root, _ = parse_object(path.read_bytes(), 0)
except Exception:
    raise SystemExit(1)

shortcuts = root.get("shortcuts", (0, {}))[1]
for _, (_, entry) in shortcuts.items():
    if all(entry.get(key, (None, None))[1] == value for key, value in expected.items()):
        raise SystemExit(0)
raise SystemExit(1)
PY
    then
      shopt -u nullglob
      return 0
    fi
  done
  shopt -u nullglob
  return 1
}

steam_shortcut_prompt() {
  local target_path="$1"
  local default="no"

  if [[ "$IS_STEAMOS" -eq 1 ]]; then
    default="yes"
  fi

  if [[ "${CDDOCK_ADD_STEAM:-}" == "0" ]]; then
    return 0
  fi

  if [[ "${CDDOCK_ADD_STEAM:-}" != "1" ]] && steam_shortcut_current "$target_path"; then
    msg "Steam 中已存在当前 CDDock 快捷方式，跳过添加。" \
      "The current CDDock Steam shortcut already exists; skipping."
    return 0
  fi

  if [[ "${CDDOCK_ADD_STEAM:-}" == "1" ]] || ask_yes_no "是否添加 CDDock 到 Steam 非 Steam 游戏列表？" \
    "Add CDDock to Steam as a non-Steam game?" "$default"; then
    add_steam_shortcut "$target_path" || true

    msg "CDDA 游戏快捷方式建议命名为 ${STEAM_SHORTCUT_RECOMMENDED_NAME}，以匹配社区控制器布局。" \
      "The CDDA game shortcut should be named ${STEAM_SHORTCUT_RECOMMENDED_NAME} to match community controller layouts."
  fi
}

main() {
  choose_language
  detect_platform

  msg "为了原生运行 CDDA tiles，可能需要安装 sdl2、sdl2_image、sdl2_mixer、sdl2_ttf。" \
    "Running CDDA tiles natively may require sdl2, sdl2_image, sdl2_mixer, and sdl2_ttf."

  if [[ "$INSTALL_DEPS" == "1" && ( "$IS_STEAMOS" -eq 1 || "$IS_ARCH" -eq 1 ) ]]; then
    install_arch_sdl_packages
  elif [[ "$INSTALL_DEPS" == "1" && "$(uname -s)" == "Darwin" ]]; then
    install_macos_sdl_packages
  fi

  install_binary
  steam_shortcut_prompt "$INSTALLED_BINARY_PATH"

  msg "完成。" "Done."
}

main "$@"
