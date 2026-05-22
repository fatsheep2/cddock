use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::paths::{build_dir, versions_dir};

#[derive(Debug, Clone)]
pub struct InstalledBuild {
    pub id: String,
    pub has_executable: bool,
}

pub fn scan_installed(game_root: &Path) -> std::io::Result<Vec<InstalledBuild>> {
    let versions = versions_dir(game_root);
    if !versions.exists() {
        return Ok(Vec::new());
    }

    let mut builds = Vec::new();
    for entry in fs::read_dir(&versions)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') {
            continue;
        }
        let has_executable = find_executable(&entry.path()).is_some();
        builds.push(InstalledBuild { id, has_executable });
    }

    builds.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(builds)
}

pub fn find_executable(build_path: &Path) -> Option<PathBuf> {
    let names = platform_executable_names();
    for name in names {
        let direct = build_path.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        #[cfg(target_os = "macos")]
        if direct.is_dir()
            && direct.extension().and_then(|ext| ext.to_str()) == Some("app")
            && let Some(binary) = find_app_bundle_executable(&direct)
        {
            return Some(binary);
        }
    }
    find_executable_recursive(build_path, 0)
}

fn find_executable_recursive(dir: &Path, depth: u8) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    let names = platform_executable_names();
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if names.iter().any(|name| file_name == *name) {
                return Some(path);
            }
        } else if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            #[cfg(target_os = "macos")]
            if name.ends_with(".app")
                && let Some(binary) = find_app_bundle_executable(&path)
            {
                return Some(binary);
            }
            if name == "save" || name == ".git" || name.starts_with('.') {
                continue;
            }
            if let Some(found) = find_executable_recursive(&path, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn find_app_bundle_executable(app: &Path) -> Option<PathBuf> {
    let macos = app.join("Contents").join("MacOS");
    let names = ["Cataclysm", "cataclysm-tiles", "cataclysm"];
    for name in names {
        let candidate = macos.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    fs::read_dir(macos)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_file())
}

fn platform_executable_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["cataclysm-tiles.exe", "cataclysm.exe"]
    } else if cfg!(target_os = "macos") {
        &["cataclysm-tiles", "Cataclysm.app"]
    } else {
        &["cataclysm-tiles", "cataclysm"]
    }
}

pub fn active_build_path(game_root: &Path, active_build: &str) -> Option<PathBuf> {
    if active_build.is_empty() {
        return None;
    }
    let path = build_dir(game_root, active_build);
    if path.is_dir() { Some(path) } else { None }
}

pub fn find_most_recent_world(userdata_path: &Path) -> Option<String> {
    let save_dir = userdata_path.join("save");
    if !save_dir.is_dir() {
        return None;
    }
    let mut worlds = Vec::new();
    for entry in fs::read_dir(&save_dir).ok()?.flatten() {
        if entry.file_type().ok()?.is_dir() {
            let modified = entry.metadata().ok()?.modified().ok()?;
            worlds.push((entry.file_name().to_string_lossy().into_owned(), modified));
        }
    }
    worlds.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    worlds.first().map(|(name, _)| name.clone())
}
