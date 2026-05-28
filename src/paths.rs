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
    if (path == "~\\" || path.starts_with("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(path.trim_start_matches('~').trim_start_matches('\\'));
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

/// Shared userdata for all builds (config, save, mods, gfx, sound, ...).
pub fn shared_userdata_dir(game_root: &Path) -> PathBuf {
    game_root.join("userdata")
}

/// All builds launch with the same userdata directory regardless of channel.
pub fn userdata_dir(game_root: &Path, _channel: &str) -> PathBuf {
    shared_userdata_dir(game_root)
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

pub fn ensure_layout(game_root: &Path, _channel: &str) -> io::Result<()> {
    fs::create_dir_all(versions_dir(game_root))?;
    fs::create_dir_all(downloads_dir(game_root))?;
    fs::create_dir_all(backups_dir(game_root))?;
    ensure_userdata_layout(&shared_userdata_dir(game_root))?;
    Ok(())
}

pub fn ensure_userdata_layout(userdata: &Path) -> io::Result<()> {
    for name in SHARED_USER_DIRS {
        fs::create_dir_all(userdata.join(name))?;
    }
    Ok(())
}

/// Move user dirs out of a fresh build into shared userdata (binaries-only install).
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

/// Merge legacy layouts and per-channel userdata into unified `userdata/`.
pub fn consolidate_userdata(game_root: &Path) -> io::Result<bool> {
    let target = shared_userdata_dir(game_root);
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

    for channel in ["stable", "experimental"] {
        let channel_dir = game_root.join(format!("userdata-{channel}"));
        if !channel_dir.is_dir() {
            continue;
        }
        for name in SHARED_USER_DIRS {
            let src = channel_dir.join(name);
            if src.is_dir() {
                migrate_tree_into(&src, &target.join(name))?;
                migrated = true;
            }
        }
        remove_dir_if_empty(&channel_dir);
    }

    let versions = versions_dir(game_root);
    if versions.is_dir() {
        for entry in fs::read_dir(versions)?.flatten() {
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let build_path = entry.path();
            for name in SHARED_USER_DIRS {
                let src = build_path.join(name);
                if src.is_dir() {
                    migrate_tree_into(&src, &target.join(name))?;
                    let _ = fs::remove_dir_all(&src);
                    migrated = true;
                }
            }
        }
    }

    Ok(migrated)
}

fn remove_dir_if_empty(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    if entries.flatten().next().is_some() {
        return;
    }
    let _ = fs::remove_dir(dir);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userdata_dir_is_shared_across_channels() {
        let root = PathBuf::from("/tmp/cddock-test");
        assert_eq!(userdata_dir(&root, "stable"), root.join("userdata"));
        assert_eq!(userdata_dir(&root, "experimental"), root.join("userdata"));
    }
}
