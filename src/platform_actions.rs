use std::process::Command;

const SDL_PACKAGES: &[&str] = &["sdl2", "sdl2_image", "sdl2_mixer", "sdl2_ttf"];
const SDL_PKG_CONFIG_NAMES: &[&str] = &["sdl2", "SDL2_image", "SDL2_mixer", "SDL2_ttf"];
const SDL_LIBRARY_NAMES: &[&str] = &["libSDL2", "libSDL2_image", "libSDL2_mixer", "libSDL2_ttf"];

pub fn native_dependency_report() -> String {
    if cfg!(windows) {
        return "Windows builds usually bundle the required SDL runtime libraries.".to_string();
    }

    let missing = missing_sdl_packages();
    if missing.is_empty() {
        return "Native SDL dependencies look present.".to_string();
    }

    format!(
        "Missing or undetected: {}. Install hint: {}",
        missing.join(", "),
        native_dependency_install_hint()
    )
}

pub fn native_dependency_install_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew install sdl2 sdl2_image sdl2_mixer sdl2_ttf"
    } else if cfg!(target_os = "linux") {
        "sudo pacman -S --needed sdl2 sdl2_image sdl2_mixer sdl2_ttf"
    } else {
        "No native dependency install command is configured for this platform"
    }
}

pub fn steam_shortcut_report(binary_path: &str, shortcut_name: &str, use_konsole: bool) -> String {
    if !cfg!(target_os = "linux") {
        return "Steam shortcut writing is only supported by the installer on Linux/SteamOS for now."
            .to_string();
    }

    format!(
        "Steam shortcut: {shortcut_name}. Launch command: {}. Installer can write it via CDDOCK_ADD_STEAM=1.",
        steam_launch_command(binary_path, use_konsole)
    )
}

fn missing_sdl_packages() -> Vec<&'static str> {
    if has_command("pkg-config") {
        return SDL_PACKAGES
            .iter()
            .zip(SDL_PKG_CONFIG_NAMES.iter())
            .filter_map(|(package, pkg_config_name)| {
                let found = Command::new("pkg-config")
                    .arg("--exists")
                    .arg(pkg_config_name)
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false);
                (!found).then_some(*package)
            })
            .collect();
    }

    if cfg!(target_os = "linux") && has_command("ldconfig") {
        let output = Command::new("ldconfig").arg("-p").output();
        let text = output
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
            .unwrap_or_default();
        return SDL_PACKAGES
            .iter()
            .zip(SDL_LIBRARY_NAMES.iter())
            .filter_map(|(package, library)| (!text.contains(library)).then_some(*package))
            .collect();
    }

    if cfg!(target_os = "macos") && has_command("brew") {
        return SDL_PACKAGES
            .iter()
            .filter_map(|package| {
                let found = Command::new("brew")
                    .arg("--prefix")
                    .arg(package)
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false);
                (!found).then_some(*package)
            })
            .collect();
    }

    SDL_PACKAGES.to_vec()
}

fn has_command(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn steam_launch_command(binary_path: &str, use_konsole: bool) -> String {
    if use_konsole {
        format!("konsole --fullscreen -e {binary_path}")
    } else {
        binary_path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_launch_command_wraps_konsole_when_enabled() {
        assert_eq!(
            steam_launch_command("/home/deck/.local/bin/cddock", true),
            "konsole --fullscreen -e /home/deck/.local/bin/cddock"
        );
        assert_eq!(
            steam_launch_command("/home/deck/.local/bin/cddock", false),
            "/home/deck/.local/bin/cddock"
        );
    }
}
