use std::{
    env,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use serde::Deserialize;
use tar::Archive;
use zip::ZipArchive;

use crate::{
    builds::find_executable,
    paths::{
        build_dir, downloads_dir, ensure_layout, promote_userdata_from_build, userdata_dir,
        versions_dir,
    },
    platform::{detect_arch, detect_os, pick_best_asset_name},
};

const CDDA_REPO: &str = "CleverRaven/Cataclysm-DDA";
const CDDA_RELEASES_API: &str = "https://api.github.com/repos/CleverRaven/Cataclysm-DDA/releases";
pub const RELEASES_PER_PAGE: u32 = 50;
const USER_AGENT: &str = "cddock/0.1.0";

#[derive(Debug, Clone)]
pub struct ReleasePage {
    pub items: Vec<ReleaseOption>,
    pub page: u32,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub struct ReleaseOption {
    pub build_id: String,
    pub channel: String,
    pub label: String,
    pub asset_name: String,
    pub download_url: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub enum DownloadPhase {
    Downloading { received: u64, total: Option<u64> },
    Extracting,
    Done,
    Failed(String),
}

#[derive(Debug)]
pub struct DownloadJob {
    pub phase: Arc<Mutex<DownloadPhase>>,
    cancel: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl DownloadJob {
    pub fn poll(&mut self) {
        if let Some(handle) = self.handle.take()
            && handle.is_finished()
        {
            let _ = handle.join();
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn phase(&self) -> DownloadPhase {
        self.phase
            .lock()
            .map(|p| p.clone())
            .unwrap_or_else(|_| DownloadPhase::Failed("progress lock poisoned".to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    name: String,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct GhRef {
    #[serde(rename = "ref")]
    ref_name: String,
}

pub fn fetch_release_page(channel: &str, page: u32) -> Result<ReleasePage, String> {
    if channel == "stable" {
        let items = fetch_stable_release_options()?;
        return Ok(ReleasePage {
            items,
            page: 1,
            has_more: false,
        });
    }
    fetch_experimental_page(page)
}

pub fn start_download(game_root: &Path, release: ReleaseOption) -> Result<DownloadJob, String> {
    let phase = Arc::new(Mutex::new(DownloadPhase::Downloading {
        received: 0,
        total: Some(release.size_bytes),
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let phase_worker = Arc::clone(&phase);
    let cancel_worker = Arc::clone(&cancel);
    let game_root = game_root.to_path_buf();

    let handle = thread::spawn(move || {
        if let Err(error) = run_download(&game_root, &release, &phase_worker, &cancel_worker) {
            *phase_worker.lock().expect("phase lock") = DownloadPhase::Failed(error);
        }
    });

    Ok(DownloadJob {
        phase,
        cancel,
        handle: Some(handle),
    })
}

pub fn fetch_experimental_page(page: u32) -> Result<ReleasePage, String> {
    let releases = fetch_releases_page(page, RELEASES_PER_PAGE)?;
    let has_more = releases.len() == RELEASES_PER_PAGE as usize;
    let mut items = Vec::new();

    for release in releases {
        if !(release.prerelease || release.tag_name.contains("experimental")) {
            continue;
        }
        if let Some(option) = release_to_option(release, "experimental") {
            items.push(option);
        }
    }

    if page == 1 && items.is_empty() && !has_more {
        return Err(
            "No experimental releases with a compatible asset were found for this platform"
                .to_string(),
        );
    }

    Ok(ReleasePage {
        items,
        page,
        has_more,
    })
}

fn fetch_stable_release_options() -> Result<Vec<ReleaseOption>, String> {
    let client = http_client()?;
    let url = format!("https://api.github.com/repos/{CDDA_REPO}/git/matching-refs/tags/0.");
    let response = client
        .get(&url)
        .send()
        .map_err(|error| format!("GitHub refs request failed: {error}"))?;

    if !response.status().is_success() {
        return fetch_stable_from_release_list();
    }

    let refs: Vec<GhRef> = response
        .json()
        .map_err(|error| format!("Failed to parse GitHub refs: {error}"))?;

    let mut tags = Vec::new();
    for gh_ref in refs {
        let tag = gh_ref.ref_name.replace("refs/tags/", "");
        if tag.contains("experimental") || tag.contains("RC") {
            continue;
        }
        let parts: Vec<_> = tag.split('.').collect();
        if parts.len() == 2 && parts[0] == "0" {
            let letter = parts[1].split('-').next().unwrap_or("");
            if letter.len() == 1 && letter.chars().all(|ch| ch.is_ascii_alphabetic()) {
                tags.push(tag);
            }
        } else if tag.starts_with("0.") && tag.len() == 3 {
            let letter = &tag[2..3];
            if letter.chars().all(|ch| ch.is_ascii_alphabetic()) {
                tags.push(tag);
            }
        }
    }

    let mut options = Vec::new();
    for tag in tags.into_iter().rev() {
        let Ok(release) = fetch_release_by_tag(&client, &tag) else {
            continue;
        };
        if let Some(option) = release_to_option(release, "stable") {
            options.push(option);
        }
    }

    if options.is_empty() {
        return fetch_stable_from_release_list();
    }

    Ok(options)
}

fn fetch_stable_from_release_list() -> Result<Vec<ReleaseOption>, String> {
    let releases = fetch_releases_page(1, 100)?;
    let mut options = Vec::new();
    for release in releases {
        if release.prerelease || release.tag_name.contains("experimental") {
            continue;
        }
        if let Some(option) = release_to_option(release, "stable") {
            options.push(option);
        }
        if options.len() >= 12 {
            break;
        }
    }

    if options.is_empty() {
        return Err(
            "No stable releases with a compatible asset were found for this platform".to_string(),
        );
    }

    Ok(options)
}

fn fetch_releases_page(page: u32, per_page: u32) -> Result<Vec<GhRelease>, String> {
    let client = http_client()?;
    let response = client
        .get(CDDA_RELEASES_API)
        .query(&[
            ("per_page", per_page.to_string()),
            ("page", page.to_string()),
        ])
        .send()
        .map_err(|error| format!("GitHub API request failed: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "GitHub API returned HTTP {}",
            response.status().as_u16()
        ));
    }

    response
        .json()
        .map_err(|error| format!("Failed to parse GitHub releases: {error}"))
}

fn fetch_release_by_tag(client: &Client, tag: &str) -> Result<GhRelease, String> {
    let url = format!("https://api.github.com/repos/{CDDA_REPO}/releases/tags/{tag}");
    let response = client
        .get(&url)
        .send()
        .map_err(|error| format!("GitHub release request failed: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("Release {tag} returned HTTP {}", response.status()));
    }

    response
        .json()
        .map_err(|error| format!("Failed to parse release {tag}: {error}"))
}

fn release_to_option(release: GhRelease, channel: &str) -> Option<ReleaseOption> {
    let asset = pick_platform_asset(&release.assets)?;
    let build_id = release.tag_name.clone();
    Some(ReleaseOption {
        build_id: build_id.clone(),
        channel: channel.to_string(),
        label: format!("{} ({})", release.name, asset.name),
        asset_name: asset.name,
        download_url: asset.browser_download_url,
        size_bytes: asset.size,
    })
}

fn run_download(
    game_root: &Path,
    release: &ReleaseOption,
    phase: &Arc<Mutex<DownloadPhase>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    ensure_layout(game_root, &release.channel).map_err(|error| error.to_string())?;
    let userdata = userdata_dir(game_root, &release.channel);

    let build_path = build_dir(game_root, &release.build_id);
    if build_path.exists() {
        if find_executable(&build_path).is_none() && is_dir_empty(&build_path) {
            fs::remove_dir_all(&build_path).map_err(|error| {
                format!(
                    "Failed to remove incomplete build {}: {error}",
                    build_path.display()
                )
            })?;
        } else {
            return Err(format!(
                "Build {} is already installed at {}",
                release.build_id,
                build_path.display()
            ));
        }
    }
    let staging_path = staging_build_dir(game_root, &release.build_id);
    if staging_path.exists() {
        fs::remove_dir_all(&staging_path).map_err(|error| {
            format!(
                "Failed to remove stale staging build {}: {error}",
                staging_path.display()
            )
        })?;
    }

    fs::create_dir_all(downloads_dir(game_root)).map_err(|error| error.to_string())?;
    let archive_path = downloads_dir(game_root).join(&release.asset_name);
    let partial_archive_path = archive_path.with_extension(format!(
        "{}part",
        archive_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!("{extension}."))
            .unwrap_or_default()
    ));

    if let Err(error) = download_file(
        &release.download_url,
        &partial_archive_path,
        release.size_bytes,
        phase,
        cancel,
    ) {
        let _ = fs::remove_file(&partial_archive_path);
        return Err(error);
    }

    if cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_file(&partial_archive_path);
        return Err("Download cancelled".to_string());
    }
    if archive_path.exists() {
        fs::remove_file(&archive_path).map_err(|error| {
            format!(
                "Failed to replace existing archive {}: {error}",
                archive_path.display()
            )
        })?;
    }
    fs::rename(&partial_archive_path, &archive_path).map_err(|error| {
        format!(
            "Failed to finalize archive {}: {error}",
            archive_path.display()
        )
    })?;

    *phase.lock().expect("phase lock") = DownloadPhase::Extracting;
    fs::create_dir_all(&staging_path).map_err(|error| error.to_string())?;
    if let Err(error) = extract_archive(&archive_path, &staging_path) {
        let _ = fs::remove_dir_all(&staging_path);
        return Err(error);
    }
    let _ = fs::remove_file(&archive_path);

    let content_root = if find_executable(&staging_path).is_some() {
        staging_path.clone()
    } else if let Some(nested) = find_single_top_level_dir(&staging_path) {
        for entry in fs::read_dir(&nested).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let target = staging_path.join(entry.file_name());
            if target.exists() {
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
                } else {
                    fs::remove_file(&target).map_err(|error| error.to_string())?;
                }
            }
            fs::rename(entry.path(), &target).map_err(|error| error.to_string())?;
        }
        fs::remove_dir_all(&nested).ok();
        staging_path.clone()
    } else {
        staging_path.clone()
    };

    if find_executable(&content_root).is_none() {
        let _ = fs::remove_dir_all(&staging_path);
        return Err(format!(
            "No CDDA executable found after extracting {}",
            release.asset_name
        ));
    }

    if let Err(error) = promote_userdata_from_build(&userdata, &content_root) {
        let _ = fs::remove_dir_all(&staging_path);
        return Err(error);
    }

    fs::rename(&staging_path, &build_path).map_err(|error| {
        let _ = fs::remove_dir_all(&staging_path);
        format!(
            "Failed to finalize build {} at {}: {error}",
            release.build_id,
            build_path.display()
        )
    })?;

    *phase.lock().expect("phase lock") = DownloadPhase::Done;
    Ok(())
}

fn staging_build_dir(game_root: &Path, build_id: &str) -> PathBuf {
    versions_dir(game_root).join(format!(".partial-{}", safe_path_component(build_id)))
}

fn safe_path_component(value: &str) -> String {
    let safe = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "build".to_string()
    } else {
        safe
    }
}

fn download_file(
    url: &str,
    destination: &Path,
    total_hint: u64,
    phase: &Arc<Mutex<DownloadPhase>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let client = http_client()?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| format!("Download request failed: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("Download returned HTTP {}", response.status()));
    }

    let total = response
        .content_length()
        .or(Some(total_hint).filter(|size| *size > 0));
    let mut file =
        File::create(destination).map_err(|error| format!("Failed to create archive: {error}"))?;
    let mut received = 0u64;
    let mut buffer = [0u8; 64 * 1024];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Download cancelled".to_string());
        }

        let read = response
            .read(&mut buffer)
            .map_err(|error| format!("Download read failed: {error}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| format!("Failed to write archive: {error}"))?;
        received += read as u64;
        *phase.lock().expect("phase lock") = DownloadPhase::Downloading { received, total };
    }

    Ok(())
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), String> {
    let name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive, destination)
    } else if name.ends_with(".zip") {
        extract_zip(archive, destination)
    } else if name.ends_with(".dmg") {
        extract_dmg(archive, destination)
    } else {
        Err(format!("Unsupported archive format: {}", archive.display()))
    }
}

#[cfg(target_os = "macos")]
fn extract_dmg(archive: &Path, destination: &Path) -> Result<(), String> {
    use std::process::Command;

    let output = Command::new("hdiutil")
        .arg("attach")
        .arg("-nobrowse")
        .arg("-readonly")
        .arg("-plist")
        .arg(archive)
        .output()
        .map_err(|error| format!("Failed to run hdiutil attach: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to attach dmg: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let plist = String::from_utf8_lossy(&output.stdout);
    let Some(mount_point) = parse_mount_point(&plist) else {
        return Err("Failed to find mounted dmg path".to_string());
    };

    let copy_result = copy_mounted_dmg(&mount_point, destination);
    let detach_result = Command::new("hdiutil")
        .arg("detach")
        .arg(&mount_point)
        .arg("-quiet")
        .status()
        .map_err(|error| format!("Failed to run hdiutil detach: {error}"));

    if let Err(error) = detach_result {
        return Err(error);
    }
    copy_result
}

#[cfg(not(target_os = "macos"))]
fn extract_dmg(_archive: &Path, _destination: &Path) -> Result<(), String> {
    Err("DMG archives can only be installed on macOS".to_string())
}

#[cfg(target_os = "macos")]
fn parse_mount_point(plist: &str) -> Option<String> {
    let mut saw_mount_key = false;
    for line in plist.lines() {
        if line.contains("<key>mount-point</key>") {
            saw_mount_key = true;
            continue;
        }
        if saw_mount_key {
            let value = line
                .trim()
                .strip_prefix("<string>")?
                .strip_suffix("</string>")?;
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn copy_mounted_dmg(mount_point: &str, destination: &Path) -> Result<(), String> {
    let mount = PathBuf::from(mount_point);
    let app = fs::read_dir(&mount)
        .map_err(|error| format!("Failed to read mounted dmg: {error}"))?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .ok_or_else(|| format!("No .app bundle found in {}", mount.display()))?;

    let target = destination.join(
        app.file_name()
            .ok_or_else(|| "Mounted app has no file name".to_string())?,
    );
    crate::paths::copy_dir_all(&app, &target)
        .map_err(|error| format!("Failed to copy app bundle: {error}"))
}

fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| error.to_string())?;
    let decoder = GzDecoder::new(file);
    let mut tar = Archive::new(decoder);
    tar.unpack(destination)
        .map_err(|error| format!("Failed to extract tar.gz: {error}"))
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| error.to_string())?;
    let mut zip = ZipArchive::new(file).map_err(|error| error.to_string())?;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(|error| error.to_string())?;
        let Some(relative) = file.enclosed_name() else {
            continue;
        };
        let outpath = destination.join(relative);
        if file.is_dir() {
            fs::create_dir_all(&outpath).map_err(|error| error.to_string())?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut outfile = File::create(&outpath).map_err(|error| error.to_string())?;
            io::copy(&mut file, &mut outfile).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn find_single_top_level_dir(root: &Path) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        if entry.file_type().ok()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    if dirs.len() == 1 {
        Some(dirs.remove(0))
    } else {
        None
    }
}

fn is_dir_empty(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

fn pick_platform_asset(assets: &[GhAsset]) -> Option<GhAsset> {
    let os = detect_os();
    let arch = detect_arch();
    let picked = pick_best_asset_name(
        assets.iter().map(|asset| asset.name.as_str()),
        os,
        arch,
        true,
    )?;
    assets.iter().find(|asset| asset.name == picked).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_build_dir_uses_safe_hidden_name() {
        let root = PathBuf::from("/tmp/cddock");
        assert_eq!(
            staging_build_dir(&root, "cdda/experimental:2026").file_name(),
            Some(std::ffi::OsStr::new(".partial-cdda_experimental_2026"))
        );
    }

    #[test]
    fn stable_release_list_is_not_empty() {
        if std::env::var("CDDOCK_LIVE_TESTS").ok().as_deref() != Some("1") {
            eprintln!("skipping live GitHub test; set CDDOCK_LIVE_TESTS=1 to run");
            return;
        }
        let items = fetch_stable_release_options().expect("stable releases");
        assert!(
            !items.is_empty(),
            "expected at least one stable build with a compatible asset"
        );
    }
}

fn http_client() -> Result<Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        "application/vnd.github+json".parse().unwrap(),
    );
    if let Ok(token) = env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
        }
    }

    Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .build()
        .map_err(|error| format!("Failed to create HTTP client: {error}"))
}
