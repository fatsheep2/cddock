use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    process::{Command, Stdio},
};

use crate::{builds::find_executable, paths::logs_dir, paths::versions_dir};

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

    #[cfg(target_os = "linux")]
    {
        // Work around SDL text-input bugs when CDDA is started from a Steam shortcut.
        command.env("SteamDeck", "0");
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

pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|output| {
                let text = String::from_utf8_lossy(&output.stdout);
                text.contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
}

/// Stop tracked and any other CDDA processes launched from this game library.
pub fn stop_cdda_instances(game_root: &Path, tracked_pid: Option<u32>) -> Result<u32, String> {
    let mut stopped = 0u32;
    let mut seen = std::collections::HashSet::new();

    if let Some(pid) = tracked_pid
        && is_process_alive(pid)
    {
        if stop_game(pid).is_ok() {
            seen.insert(pid);
            stopped += 1;
        }
    }

    #[cfg(unix)]
    {
        let versions = versions_dir(game_root);
        let pattern = versions.to_string_lossy().to_string();
        if !pattern.is_empty() {
            if let Ok(output) = Command::new("pgrep").arg("-f").arg(&pattern).output() {
                for line in output.stdout.split(|byte| *byte == b'\n') {
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(text) = std::str::from_utf8(line) else {
                        continue;
                    };
                    let Ok(pid) = text.trim().parse::<u32>() else {
                        continue;
                    };
                    if seen.contains(&pid) || !is_process_alive(pid) {
                        continue;
                    }
                    if stop_game(pid).is_ok() {
                        seen.insert(pid);
                        stopped += 1;
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        let versions = versions_dir(game_root);
        let versions_text = versions.to_string_lossy().to_lowercase();
        for image in ["cataclysm-tiles.exe", "cataclysm.exe"] {
            let Ok(output) = Command::new("tasklist")
                .args(["/FI", &format!("IMAGENAME eq {image}"), "/FO", "CSV", "/NH"])
                .output()
            else {
                continue;
            };
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let Some(pid_field) = line.split(',').nth(1) else {
                    continue;
                };
                let pid_text = pid_field.trim_matches('"');
                let Ok(pid) = pid_text.parse::<u32>() else {
                    continue;
                };
                if seen.contains(&pid) || !is_process_alive(pid) {
                    continue;
                }
                if let Ok(exe_output) = Command::new("wmic")
                    .args([
                        "process",
                        "where",
                        &format!("ProcessId={pid}"),
                        "get",
                        "ExecutablePath",
                        "/value",
                    ])
                    .output()
                {
                    let exe_text = String::from_utf8_lossy(&exe_output.stdout).to_lowercase();
                    if !exe_text.contains(&versions_text) {
                        continue;
                    }
                }
                if stop_game(pid).is_ok() {
                    seen.insert(pid);
                    stopped += 1;
                }
            }
        }
    }

    Ok(stopped)
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
