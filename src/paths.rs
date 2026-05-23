use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

/// Project root, e.g. `~/.local/cddock` (Catapult/catman-style launcher root).
pub fn expand_path(path: &str) -> PathBuf {
    let path = path.trim();
    if path == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().unwrap_or_else(|| PathBuf::from(path)).join(rest);
    }
    if path == "~\\" || path.starts_with("~\\") {
        if let Some(home) = home_dir() {
            return home.join(path.trim_start_matches('~').trim_start_matches('\\'));
        }
    }
    PathBuf::from(path)
}

pub fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var("USERPROFILE").ok().map(PathBuf::from)
    } else {
        env::var("HOME").ok().map(PathBuf::from)
    }
}

pub fn default_game_root() -> PathBuf {
    home_dir()
        .map(|home| home.join(".local").join("cddock"))
        .unwrap_or_else(|| PathBuf::from(".local/cddock"))
}

/// Game binaries only: `~/.local/cddock/versions/<build-tag>`.
pub fn versions_dir(game_root: &Path) -> PathBuf {
    game_root.join("versions")
}

pub fn build_dir(game_root: &Path, build_id: &str) -> PathBuf {
    versions_dir(game_root).join(build_id)
}

/// Staging downloads (catman-style).
pub fn downloads_dir(game_root: &Path) -> PathBuf {
    game_root.join("downloads")
}

/// Shared user data per release channel (catman: `userdata-stable` / `userdata-experimental`).
pub fn userdata_dir(game_root: &Path, channel: &str) -> PathBuf {
    game_root.join(format!("userdata-{channel}"))
}

pub fn legacy_userdata_dir(game_root: &Path) -> PathBuf {
    game_root.join("userdata")
}

pub fn backups_dir(game_root: &Path) -> PathBuf {
    game_root.join("backups")
}

pub fn guide_cache_dir(game_root: &Path) -> PathBuf {
    game_root.join("cache").join("guide")
}

pub fn logs_dir(game_root: &Path) -> PathBuf {
    game_root.join("logs")
}

/// Directories shared across all game installs (Catapult userdata migration set).
pub const SHARED_USER_DIRS: &[&str] = &[
    "save",
    "gfx",
    "mods",
    "sound",
    "font",
    "config",
    "memorial",
    "graveyard",
    "templates",
];

pub fn ensure_layout(game_root: &Path, channel: &str) -> io::Result<()> {
    fs::create_dir_all(versions_dir(game_root))?;
    fs::create_dir_all(downloads_dir(game_root))?;
    fs::create_dir_all(backups_dir(game_root))?;
    ensure_userdata_layout(&userdata_dir(game_root, channel))?;
    Ok(())
}

pub fn ensure_userdata_layout(userdata: &Path) -> io::Result<()> {
    for name in SHARED_USER_DIRS {
        fs::create_dir_all(userdata.join(name))?;
    }
    Ok(())
}

/// Move user dirs out of a fresh build into channel userdata (Catapult split install vs userdata).
pub fn promote_userdata_from_build(userdata: &Path, build_path: &Path) -> Result<(), String> {
    ensure_userdata_layout(userdata).map_err(|error| error.to_string())?;
    for name in SHARED_USER_DIRS {
        promote_one_dir(build_path, name, userdata.join(name))?;
    }
    Ok(())
}

fn promote_one_dir(build_path: &Path, name: &str, shared_target: PathBuf) -> Result<(), String> {
    let src = build_path.join(name);
    if !src.is_dir() {
        return Ok(());
    }

    if !shared_target.exists() {
        fs::rename(&src, &shared_target).map_err(|error| {
            format!(
                "Failed to move {} to {}: {error}",
                src.display(),
                shared_target.display()
            )
        })?;
        return Ok(());
    }

    fs::remove_dir_all(&src).map_err(|error| {
        format!(
            "Failed to remove duplicate {} from {}: {error}",
            name,
            build_path.display()
        )
    })
}

/// Migrate flat `save/` / `gfx/` at project root or legacy `userdata/` into channel userdata.
pub fn migrate_legacy_layout(game_root: &Path, channel: &str) -> io::Result<bool> {
    let target = userdata_dir(game_root, channel);
    ensure_userdata_layout(&target)?;
    let mut migrated = false;

    for name in SHARED_USER_DIRS {
        let legacy_root = game_root.join(name);
        if legacy_root.is_dir() {
            migrate_tree_into(&legacy_root, &target.join(name))?;
            let _ = fs::remove_dir_all(&legacy_root);
            migrated = true;
        }
    }

    let legacy_userdata = legacy_userdata_dir(game_root);
    if legacy_userdata.is_dir() {
        for name in SHARED_USER_DIRS {
            let src = legacy_userdata.join(name);
            if src.is_dir() {
                migrate_tree_into(&src, &target.join(name))?;
                migrated = true;
            }
        }
        if legacy_userdata.read_dir()?.flatten().count() == 0 {
            let _ = fs::remove_dir(legacy_userdata);
        }
    }

    Ok(migrated)
}

fn migrate_tree_into(src: &Path, dst: &Path) -> io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    if !dst.exists() {
        fs::rename(src, dst)?;
        return Ok(());
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
            fs::remove_dir_all(&from)?;
        } else {
            if !to.exists() {
                fs::copy(&from, &to)?;
            }
            fs::remove_file(&from)?;
        }
    }
    Ok(())
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
