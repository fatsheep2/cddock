#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsName {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

pub fn detect_os() -> OsName {
    if cfg!(target_os = "macos") {
        OsName::Macos
    } else if cfg!(windows) {
        OsName::Windows
    } else {
        OsName::Linux
    }
}

pub fn detect_arch() -> Arch {
    if cfg!(target_arch = "aarch64") {
        Arch::Arm64
    } else {
        Arch::X64
    }
}

/// Match a GitHub release asset name (logic adapted from catman `platform_util.py`).
pub fn match_asset_name(name: &str, os: OsName, arch: Arch, tiles: bool) -> bool {
    let n = name.to_ascii_lowercase();

    if !n.ends_with(".tar.gz")
        && !n.ends_with(".tar.xz")
        && !n.ends_with(".tar.bz2")
        && !n.ends_with(".zip")
        && !n.ends_with(".dmg")
    {
        return false;
    }

    if n.contains("android") || n.contains("wasm") || n.contains(".aab") || n.contains(".apk") {
        return false;
    }

    match os {
        OsName::Macos if !n.contains("macos") && !n.contains("osx") && !n.contains("darwin") => {
            return false;
        }
        OsName::Linux if !n.contains("linux") => return false,
        OsName::Windows
            if !n.contains("windows") && !n.contains("win64") && !n.contains("win32") =>
        {
            return false;
        }
        _ => {}
    }

    if tiles {
        if n.contains("curses") || n.contains("terminal-only") {
            return false;
        }
    } else if n.contains("tiles") || n.contains("with-graphics") {
        return false;
    }

    if n.contains("universal") {
        return true;
    }

    match arch {
        Arch::Arm64 => n.contains("arm64") || n.contains("aarch64") || n.contains("-arm-"),
        Arch::X64 => !(n.contains("arm64") || n.contains("aarch64") || n.contains("-arm-")),
    }
}

pub fn pick_best_asset_name<'a>(
    asset_names: impl IntoIterator<Item = &'a str>,
    os: OsName,
    arch: Arch,
    tiles: bool,
) -> Option<&'a str> {
    let names: Vec<&str> = asset_names.into_iter().collect();
    let mut matches: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| match_asset_name(name, os, arch, tiles))
        .collect();

    if matches.is_empty() && os == OsName::Macos && arch == Arch::Arm64 {
        matches = names
            .iter()
            .copied()
            .filter(|name| match_asset_name(name, os, Arch::X64, tiles))
            .collect();
    }

    if matches.is_empty() {
        return None;
    }

    let ranked: Vec<(&str, u8)> = matches
        .iter()
        .map(|name| (*name, asset_priority(name)))
        .collect();
    ranked
        .iter()
        .max_by_key(|(_, score)| *score)
        .map(|(name, _)| *name)
}

fn asset_priority(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    let mut score = 0u8;
    if n.contains("sound") {
        score = score.saturating_add(40);
    }
    if n.contains("with-graphics") || n.contains("tiles") {
        score = score.saturating_add(30);
    }
    if n.ends_with(".tar.gz") || n.ends_with(".tar.xz") || n.ends_with(".tar.bz2") {
        score = score.saturating_add(20);
    } else if n.ends_with(".zip") {
        score = score.saturating_add(10);
    }
    if n.contains("msvc") {
        score = score.saturating_sub(5);
    }
    score
}
