use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::paths::{backups_dir, userdata_dir};
use zip::write::SimpleFileOptions;

pub fn backup_saves(game_root: &Path, channel: &str) -> Result<String, String> {
    let save_dir = userdata_dir(game_root, channel).join("save");
    if !save_dir.is_dir() {
        return Err(format!("No save directory at {}", save_dir.display()));
    }

    let has_saves = fs::read_dir(&save_dir)
        .map_err(|error| format!("Failed to read save directory: {error}"))?
        .flatten()
        .next()
        .is_some();
    if !has_saves {
        return Err(format!("Save directory is empty: {}", save_dir.display()));
    }

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let backup_name = format!("save-{channel}-{timestamp}");
    let destination = backups_dir(game_root).join(format!("{backup_name}.zip"));

    fs::create_dir_all(backups_dir(game_root))
        .map_err(|error| format!("Failed to create backups directory: {error}"))?;

    if destination.exists() {
        return Err(format!("Backup already exists: {}", destination.display()));
    }

    zip_directory(&save_dir, &destination)
        .map_err(|error| format!("Failed to create backup zip: {error}"))?;

    Ok(destination.display().to_string())
}

fn zip_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    let file = File::create(destination)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in walkdir_paths(source)? {
        let relative = entry.strip_prefix(source).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
        })?;
        if entry.is_dir() {
            let name = relative.to_string_lossy().replace('\\', "/");
            let name = if name.ends_with('/') {
                name
            } else {
                format!("{name}/")
            };
            if !name.is_empty() {
                zip.add_directory(name, options)?;
            }
            continue;
        }
        let name = relative.to_string_lossy().replace('\\', "/");
        zip.start_file(name, options)?;
        let mut input = File::open(entry)?;
        let mut buffer = Vec::new();
        input.read_to_end(&mut buffer)?;
        zip.write_all(&buffer)?;
    }

    zip.finish()?;
    Ok(())
}

fn walkdir_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                paths.push(path.clone());
                stack.push(path);
            } else {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}
