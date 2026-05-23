use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    process::{Command, Stdio},
};

use crate::{builds::find_executable, paths::logs_dir};

pub fn launch_build(
    build_path: &Path,
    userdata_path: &Path,
    world: Option<&str>,
) -> Result<u32, String> {
    let executable = find_executable(build_path)
        .ok_or_else(|| format!("No CDDA executable found under {}", build_path.display()))?;
    let working_dir = executable
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", executable.display()))?;

    fs::create_dir_all(userdata_path).map_err(|error| error.to_string())?;
    let game_root = build_path
        .parent()
        .and_then(|versions| versions.parent())
        .unwrap_or(build_path);
    let log_dir = logs_dir(game_root);
    fs::create_dir_all(&log_dir).map_err(|error| error.to_string())?;
    let log_path = log_dir.join("game-launch.log");
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| format!("Failed to open launch log {}: {error}", log_path.display()))?;
    let stdout = File::options()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| format!("Failed to open launch log {}: {error}", log_path.display()))?;

    let userdir = userdata_path
        .canonicalize()
        .unwrap_or_else(|_| userdata_path.to_path_buf());

    let mut command = Command::new(&executable);
    command
        .arg("--userdir")
        .arg(&userdir)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    if let Some(world) = world {
        command.arg("--world").arg(world);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(parent) = executable.parent() {
            let parent = parent.to_string_lossy().to_string();
            command.env("DYLD_LIBRARY_PATH", &parent);
            command.env("DYLD_FRAMEWORK_PATH", &parent);
        }
    }

    let child = command
        .spawn()
        .map_err(|error| format!("Failed to launch {}: {error}", executable.display()))?;

    Ok(child.id())
}

pub fn stop_game(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        let group = format!("-{pid}");
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(&group)
            .status()
            .map_err(|error| format!("Failed to stop game process group {pid}: {error}"))?;
        if status.success() {
            return Ok(());
        }

        let status = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .map_err(|error| format!("Failed to stop game process {pid}: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Failed to stop game process {pid}"))
        }
    }

    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .status()
            .map_err(|error| format!("Failed to stop game process {pid}: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Failed to stop game process {pid}"))
        }
    }
}
