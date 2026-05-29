use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    http,
    paths::{build_dir, guide_cache_dir, shared_userdata_dir},
};

const BUILDS_URL: &str = "https://raw.githubusercontent.com/nornagon/cdda-data/main/builds.json";
const DATA_BASE_URL: &str = "https://raw.githubusercontent.com/nornagon/cdda-data/main/data";

#[derive(Debug, Clone, Deserialize)]
pub struct GuideBuild {
    pub build_number: String,
    pub prerelease: bool,
    #[serde(default)]
    #[serde(rename = "langs")]
    pub langs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GuideSearchResult {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub fields: Vec<(String, String)>,
    pub raw_json: String,
}

#[derive(Debug)]
pub struct GuideDataset {
    entries: Vec<GuideSearchResult>,
    language: String,
    warning: Option<String>,
}

pub fn guide_language(language: &str) -> &'static str {
    match language {
        "chinese" | "zh" | "zh_CN" | "zh-cn" => "zh_CN",
        _ => "en",
    }
}

pub fn resolve_build(
    game_root: &Path,
    active_build: &str,
    channel: &str,
) -> Result<String, String> {
    if !active_build.trim().is_empty() {
        return resolve_active_build(game_root, active_build.trim());
    }

    let builds = fetch_builds(game_root)?;
    if channel == "stable" {
        if let Some(build) = builds.iter().find(|build| !build.prerelease) {
            return Ok(build.build_number.clone());
        }
    }

    builds
        .iter()
        .find(|build| build.prerelease)
        .or_else(|| builds.first())
        .map(|build| build.build_number.clone())
        .ok_or_else(|| "No cdda-guide builds were found".to_string())
}

fn resolve_active_build(game_root: &Path, active_build: &str) -> Result<String, String> {
    let Ok(builds) = fetch_builds(game_root) else {
        return Ok(active_build.to_string());
    };

    if builds
        .iter()
        .any(|build| build.build_number == active_build)
    {
        return Ok(active_build.to_string());
    }

    let stable_release = format!("{active_build}-RELEASE");
    if builds
        .iter()
        .any(|build| build.build_number == stable_release)
    {
        return Ok(stable_release);
    }

    let cdda_prefixed = format!("cdda-{active_build}");
    builds
        .iter()
        .find(|build| build.build_number == cdda_prefixed)
        .or_else(|| {
            builds
                .iter()
                .find(|build| build.build_number.starts_with(&cdda_prefixed))
        })
        .map(|build| build.build_number.clone())
        .or_else(|| {
            builds
                .iter()
                .find(|build| build.build_number.starts_with(active_build))
                .map(|build| build.build_number.clone())
        })
        .or_else(|| Some(active_build.to_string()))
        .ok_or_else(|| "No cdda-guide builds were found".to_string())
}

pub fn fetch_builds(game_root: &Path) -> Result<Vec<GuideBuild>, String> {
    let cache_path = guide_cache_dir(game_root).join("builds.json");
    let text = fetch_cached(BUILDS_URL, &cache_path)?;
    serde_json::from_str(&text).map_err(|error| format!("Failed to parse guide builds: {error}"))
}

pub fn load_dataset(game_root: &Path, build: &str, language: &str) -> Result<GuideDataset, String> {
    let data = load_json(game_root, build, "all.json")?;
    let (translations, actual_language, warning) =
        load_translations_with_fallback(game_root, build, language);
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut objects = Vec::new();
    collect_objects(&data, &mut objects);
    let object_index = object_id_index(&objects, &translations);
    let resolved_objects = objects
        .iter()
        .map(|map| resolve_copy_from(map, &object_index, &translations, 0))
        .collect::<Vec<_>>();
    collect_entries(&resolved_objects, &translations, &mut seen, &mut entries);
    add_derived_fields(&resolved_objects, &mut entries, &translations);
    Ok(GuideDataset {
        entries,
        language: actual_language,
        warning,
    })
}

impl GuideDataset {
    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, id: &str) -> Option<GuideSearchResult> {
        self.entries.iter().find(|entry| entry.id == id).cloned()
    }

    pub fn contains_id(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }
}

pub fn search_dataset(dataset: &GuideDataset, query: &str, limit: usize) -> Vec<GuideSearchResult> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let query = query.to_lowercase();
    let terms = query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    let mut matches = dataset
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, result)| {
            search_score(result, &query, &terms).map(|score| (score, index, result))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(score, index, _)| (*score, *index));
    matches
        .into_iter()
        .take(limit)
        .map(|(_, _, result)| result.clone())
        .collect()
}

fn search_score(result: &GuideSearchResult, query: &str, terms: &[&str]) -> Option<usize> {
    let id = result.id.to_lowercase();
    let kind = result.kind.to_lowercase();
    let name = result.name.to_lowercase();
    let description = result.description.to_lowercase();
    if id == query {
        return Some(0);
    }
    if id.starts_with(query) {
        return Some(10);
    }
    if name == query {
        return Some(20);
    }
    if name.starts_with(query) {
        return Some(30);
    }
    if id.contains(query) {
        return Some(40);
    }
    if name.contains(query) {
        return Some(50);
    }
    if kind.contains(query) {
        return Some(60);
    }
    if description.contains(query) {
        return Some(70);
    }
    if result.fields.iter().any(|(key, value)| {
        key.to_lowercase().contains(query) || value.to_lowercase().contains(query)
    }) {
        return Some(80);
    }
    if terms.len() > 1 {
        let mut total = 0;
        for term in terms {
            total += search_term_score(result, term)?;
        }
        return Some(100 + total);
    }
    None
}

fn search_term_score(result: &GuideSearchResult, query: &str) -> Option<usize> {
    let id = result.id.to_lowercase();
    let kind = result.kind.to_lowercase();
    let name = result.name.to_lowercase();
    let description = result.description.to_lowercase();
    if id == query {
        return Some(0);
    }
    if id.starts_with(query) {
        return Some(10);
    }
    if name == query {
        return Some(20);
    }
    if name.starts_with(query) {
        return Some(30);
    }
    if id.contains(query) {
        return Some(40);
    }
    if name.contains(query) {
        return Some(50);
    }
    if kind.contains(query) {
        return Some(60);
    }
    if description.contains(query) {
        return Some(70);
    }
    if result.fields.iter().any(|(key, value)| {
        key.to_lowercase().contains(query) || value.to_lowercase().contains(query)
    }) {
        return Some(80);
    }
    None
}

pub fn relation_target_ids(result: &GuideSearchResult) -> Vec<String> {
    let mut targets = Vec::new();
    for (key, value) in &result.fields {
        if is_relation_field(key) {
            for candidate in extract_relation_ids(value) {
                push_unique_target(&mut targets, &result.id, candidate);
            }
        }
    }
    targets
}

pub fn field_target_ids(result: &GuideSearchResult) -> Vec<String> {
    let mut targets = relation_target_ids(result);
    for (key, value) in &result.fields {
        if is_tile_field(key) {
            continue;
        }
        for candidate in extract_relation_ids(value) {
            push_unique_target(&mut targets, &result.id, candidate);
        }
    }
    targets
}

fn push_unique_target(targets: &mut Vec<String>, current_id: &str, candidate: String) {
    if candidate != current_id && !targets.contains(&candidate) {
        targets.push(candidate);
    }
}

pub fn add_local_tile_info(game_root: &Path, active_build: &str, result: &mut GuideSearchResult) {
    if active_build.trim().is_empty() {
        return;
    }
    let preview_dir = guide_cache_dir(game_root)
        .join("tiles")
        .join(safe_file_name(&result.id));
    let mut matches = Vec::new();
    for candidate in tile_lookup_ids(result) {
        let candidate_matches =
            find_tile_matches(game_root, active_build, &candidate, &preview_dir);
        for item in candidate_matches {
            let item = if candidate == result.id {
                item
            } else {
                format!("matched_id: {candidate}; {item}")
            };
            if !matches.contains(&item) {
                matches.push(item);
            }
        }
        if !matches.is_empty() {
            break;
        }
    }
    if matches.is_empty() {
        result.fields.push((
            "tile_match".to_string(),
            format!(
                "no local tileset entry found for {} or looks_like ids under active build gfx/ or userdata/gfx/",
                result.id
            ),
        ));
        return;
    }

    for item in matches.into_iter().take(6) {
        result.fields.push(("tile_match".to_string(), item));
    }
}

fn tile_lookup_ids(result: &GuideSearchResult) -> Vec<String> {
    let mut ids = vec![result.id.clone()];
    for (key, value) in &result.fields {
        if matches!(key.as_str(), "looks_like" | "fallback") {
            for candidate in extract_relation_ids(value) {
                if !ids.contains(&candidate) {
                    ids.push(candidate);
                }
            }
        }
    }
    ids
}

fn find_tile_matches(
    game_root: &Path,
    active_build: &str,
    id: &str,
    preview_dir: &Path,
) -> Vec<String> {
    let mut matches = Vec::new();
    for gfx in local_gfx_dirs(game_root, active_build) {
        collect_gfx_tile_matches(&gfx, id, preview_dir, &mut matches);
    }
    matches
}

fn local_gfx_dirs(game_root: &Path, active_build: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in [
        build_dir(game_root, active_build).join("gfx"),
        shared_userdata_dir(game_root).join("gfx"),
    ] {
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn collect_gfx_tile_matches(gfx: &Path, id: &str, preview_dir: &Path, matches: &mut Vec<String>) {
    let Ok(tilesets) = fs::read_dir(gfx) else {
        return;
    };
    for entry in tilesets.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let config = entry.path().join("tile_config.json");
        if !config.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&config) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let tileset = entry.file_name().to_string_lossy().into_owned();
        let tile_size = tile_config_tile_size(&value);
        collect_tile_matches(
            &value,
            id,
            &tileset,
            &entry.path(),
            None,
            tile_size,
            preview_dir,
            matches,
        );
    }
}

fn collect_tile_matches(
    value: &Value,
    id: &str,
    tileset: &str,
    tileset_dir: &Path,
    sheet: Option<&str>,
    tile_size: Option<(u32, u32)>,
    preview_dir: &Path,
    matches: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            let current_sheet = map.get("file").and_then(|value| value.as_str()).or(sheet);
            if map
                .get("id")
                .is_some_and(|value| tile_id_matches(value, id))
            {
                matches.push(tile_match_summary(
                    map,
                    tileset,
                    tileset_dir,
                    current_sheet,
                    tile_size,
                    preview_dir,
                ));
            }
            for child in map.values() {
                collect_tile_matches(
                    child,
                    id,
                    tileset,
                    tileset_dir,
                    current_sheet,
                    tile_size,
                    preview_dir,
                    matches,
                );
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_tile_matches(
                    child,
                    id,
                    tileset,
                    tileset_dir,
                    sheet,
                    tile_size,
                    preview_dir,
                    matches,
                );
            }
        }
        _ => {}
    }
}

fn tile_id_matches(value: &Value, id: &str) -> bool {
    match value {
        Value::String(text) => text == id,
        Value::Array(values) => values.iter().any(|value| tile_id_matches(value, id)),
        _ => false,
    }
}

fn tile_match_summary(
    map: &Map<String, Value>,
    tileset: &str,
    tileset_dir: &Path,
    sheet: Option<&str>,
    tile_size: Option<(u32, u32)>,
    preview_dir: &Path,
) -> String {
    let mut parts = vec![format!("tileset: {tileset}")];
    if let Some(sheet) = sheet {
        parts.push(format!("sheet: {sheet}"));
    }
    for key in ["fg", "bg", "multitile", "rotates", "additional_tiles"] {
        if let Some(value) = map
            .get(key)
            .and_then(|value| compact_value(value, &HashMap::new()))
        {
            parts.push(format!("{key}: {value}"));
        }
    }
    if let (Some(sheet), Some((tile_width, tile_height))) = (sheet, tile_size) {
        let sheet_path = tileset_dir.join(sheet);
        if let Some((image_width, image_height)) = png_dimensions(&sheet_path) {
            let columns = (image_width / tile_width).max(1);
            for (key, tile_id) in tile_sprite_layers(map) {
                let x = (tile_id % columns) * tile_width;
                let y = (tile_id / columns) * tile_height;
                parts.push(format!("{key}_crop: {x},{y} {tile_width}x{tile_height}"));
                if x.saturating_add(tile_width) > image_width
                    || y.saturating_add(tile_height) > image_height
                {
                    parts.push(format!(
                        "{key}_preview_error: crop outside {image_width}x{image_height}"
                    ));
                    continue;
                }
                if let Some(preview) = export_tile_preview(
                    &sheet_path,
                    preview_dir,
                    tileset,
                    sheet,
                    &key,
                    x,
                    y,
                    tile_width,
                    tile_height,
                ) {
                    parts.push(format!("{key}_preview: {}", preview.display()));
                }
            }
        }
    }
    parts.join("; ")
}

fn tile_sprite_layers(map: &Map<String, Value>) -> Vec<(String, u32)> {
    let mut layers = Vec::new();
    for key in ["fg", "bg"] {
        if let Some(tile_id) = map.get(key).and_then(first_tile_sprite_index) {
            layers.push((key.to_string(), tile_id));
        }
    }
    if let Some(additional) = map.get("additional_tiles") {
        collect_additional_tile_layers(additional, &mut layers);
    }
    layers
}

fn collect_additional_tile_layers(value: &Value, layers: &mut Vec<(String, u32)>) {
    match value {
        Value::Array(values) => {
            for child in values {
                collect_additional_tile_layers(child, layers);
            }
        }
        Value::Object(map) => {
            let suffix = map
                .get("id")
                .and_then(|value| compact_value(value, &HashMap::new()))
                .unwrap_or_else(|| "additional".to_string());
            for key in ["fg", "bg"] {
                if let Some(tile_id) = map.get(key).and_then(first_tile_sprite_index) {
                    layers.push((format!("additional_{suffix}_{key}"), tile_id));
                }
            }
            for child in map.values() {
                if child.is_object() || child.is_array() {
                    collect_additional_tile_layers(child, layers);
                }
            }
        }
        _ => {}
    }
}

fn first_tile_sprite_index(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::Array(values) => values.iter().find_map(first_tile_sprite_index),
        Value::Object(map) => map.values().find_map(first_tile_sprite_index),
        _ => None,
    }
}

fn export_tile_preview(
    sheet_path: &Path,
    preview_dir: &Path,
    tileset: &str,
    sheet: &str,
    layer: &str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Option<PathBuf> {
    let image = image::open(sheet_path).ok()?;
    let crop = image.crop_imm(x, y, width, height);
    fs::create_dir_all(preview_dir).ok()?;
    let filename = format!(
        "{}-{}-{}-{}-{}.png",
        safe_file_name(tileset),
        safe_file_name(sheet),
        safe_file_name(layer),
        x,
        y
    );
    let destination = preview_dir.join(filename);
    crop.save(&destination).ok()?;
    Some(destination)
}

fn tile_config_tile_size(value: &Value) -> Option<(u32, u32)> {
    let tile_info = value.get("tile_info")?.as_array()?.first()?.as_object()?;
    let width = tile_info.get("width")?.as_u64()? as u32;
    let height = tile_info.get("height")?.as_u64()? as u32;
    if width == 0 || height == 0 {
        None
    } else {
        Some((width, height))
    }
}

fn png_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut file = fs::File::open(path).ok()?;
    let mut header = [0u8; 24];
    file.read_exact(&mut header).ok()?;
    if &header[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    Some((width, height))
}

fn safe_file_name(value: &str) -> String {
    let safe = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "tile".to_string()
    } else {
        safe
    }
}

fn load_translations_with_fallback(
    game_root: &Path,
    build: &str,
    language: &str,
) -> (HashMap<String, String>, String, Option<String>) {
    if language == "en" {
        return (HashMap::new(), "en".to_string(), None);
    }

    match build_has_language(game_root, build, language) {
        Ok(false) => {
            if let Ok(translations) = load_local_translations(game_root, build, language) {
                return (
                    translations,
                    language.to_string(),
                    Some(format!(
                        "Guide language {language} is not listed for {build}; using installed game translations."
                    )),
                );
            }
            return (
                HashMap::new(),
                "en".to_string(),
                Some(format!(
                    "Guide language {language} is not available for {build}; using English."
                )),
            );
        }
        Ok(true) => {}
        Err(error) => {
            return match load_translations(game_root, build, language) {
                Ok(translations) => (
                    translations,
                    language.to_string(),
                    Some(format!("Could not verify guide language list: {error}")),
                ),
                Err(load_error) => (
                    HashMap::new(),
                    "en".to_string(),
                    Some(format!(
                        "Guide language {language} could not be loaded for {build}; using English. {load_error}"
                    )),
                ),
            };
        }
    }

    match load_translations(game_root, build, language) {
        Ok(translations) => (translations, language.to_string(), None),
        Err(error) => (
            HashMap::new(),
            "en".to_string(),
            Some(format!(
                "Guide language {language} could not be loaded for {build}; using English. {error}"
            )),
        ),
    }
}

fn build_has_language(game_root: &Path, build: &str, language: &str) -> Result<bool, String> {
    let builds = fetch_builds(game_root)?;
    let Some(item) = builds.iter().find(|item| item.build_number == build) else {
        return Ok(true);
    };
    Ok(item.langs.iter().any(|lang| lang == language))
}
fn load_json(game_root: &Path, build: &str, relative: &str) -> Result<Value, String> {
    if relative == "all.json"
        && let Ok(value) = load_local_all_json(game_root, build)
    {
        return Ok(value);
    }

    let url = format!("{DATA_BASE_URL}/{build}/{relative}");
    let cache_path = guide_cache_dir(game_root).join(build).join(relative);
    let text = fetch_cached(&url, &cache_path)?;
    serde_json::from_str(&text).map_err(|error| {
        format!(
            "Failed to parse guide data {} for {}: {error}",
            relative, build
        )
    })
}

fn load_local_all_json(game_root: &Path, build: &str) -> Result<Value, String> {
    let mut errors = Vec::new();
    let mut values = Vec::new();
    for data_dir in local_data_dirs(game_root, build) {
        let mut files = Vec::new();
        collect_json_files(&data_dir, &mut files).map_err(|error| {
            format!(
                "Failed to read local data dir {}: {error}",
                data_dir.display()
            )
        })?;
        files.sort();

        for file in files {
            match fs::read_to_string(&file)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())
                }) {
                Ok(Value::Array(items)) => values.extend(
                    items
                        .into_iter()
                        .map(|value| attach_source_file(value, &file, game_root)),
                ),
                Ok(value @ Value::Object(_)) => {
                    values.push(attach_source_file(value, &file, game_root))
                }
                Ok(_) => {}
                Err(error) => {
                    if errors.len() < 3 {
                        errors.push(format!("{}: {error}", file.display()));
                    }
                }
            }
        }
    }

    if !values.is_empty() {
        return Ok(Value::Array(values));
    }

    if errors.is_empty() {
        Err(format!("No local CDDA data/json found for {build}"))
    } else {
        Err(format!(
            "No parseable local CDDA JSON found for {build}: {}",
            errors.join("; ")
        ))
    }
}

fn local_data_dirs(game_root: &Path, build: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut candidates = vec![build.to_string()];
    if let Some(stripped) = build.strip_suffix("-RELEASE") {
        candidates.push(stripped.to_string());
    }
    if let Some(stripped) = build.strip_prefix("cdda-") {
        candidates.push(stripped.to_string());
    }

    for candidate in candidates {
        push_local_data_dirs(&mut dirs, &build_dir(game_root, &candidate));
    }

    let versions = game_root.join("versions");
    if let Ok(entries) = fs::read_dir(versions) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == build || name.starts_with(build) || build.starts_with(&name) {
                push_local_data_dirs(&mut dirs, &entry.path());
            }
        }
    }

    dirs
}

fn push_local_data_dirs(dirs: &mut Vec<PathBuf>, build_path: &Path) {
    for data in [
        build_path.join("data").join("json"),
        build_path.join("data").join("mods"),
    ] {
        if data.is_dir() && !dirs.contains(&data) {
            dirs.push(data);
        }
    }
}

fn attach_source_file(mut value: Value, file: &Path, game_root: &Path) -> Value {
    if let Value::Object(map) = &mut value {
        let source = file
            .strip_prefix(game_root)
            .unwrap_or(file)
            .display()
            .to_string();
        map.entry("_source_file".to_string())
            .or_insert_with(|| Value::String(source));
    }
    value
}

fn collect_json_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_json_files(&entry.path(), files)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn load_translations(
    game_root: &Path,
    build: &str,
    language: &str,
) -> Result<HashMap<String, String>, String> {
    if language == "en" {
        return Ok(HashMap::new());
    }

    if let Ok(translations) = load_local_translations(game_root, build, language) {
        return Ok(translations);
    }

    let url = format!("{DATA_BASE_URL}/{build}/lang/{language}.json");
    let cache_path = guide_cache_dir(game_root)
        .join(build)
        .join("lang")
        .join(format!("{language}.json"));
    let text = fetch_cached(&url, &cache_path)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("Failed to parse guide language file: {error}"))?;
    let mut translations = HashMap::new();
    if let Value::Object(map) = value {
        for (key, value) in map {
            match value {
                Value::String(text) => {
                    translations.insert(key, text);
                }
                Value::Array(values) => {
                    if let Some(Value::String(text)) = values.first() {
                        translations.insert(key, text.clone());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(translations)
}

fn load_local_translations(
    game_root: &Path,
    build: &str,
    language: &str,
) -> Result<HashMap<String, String>, String> {
    for path in local_translation_paths(game_root, build, language) {
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|error| {
            format!(
                "Failed to read local translation {}: {error}",
                path.display()
            )
        })?;
        let translations = parse_po_translations(&text);
        if !translations.is_empty() {
            return Ok(translations);
        }
    }
    Err(format!(
        "No local {language} translation file found for {build}"
    ))
}

fn local_translation_paths(game_root: &Path, build: &str, language: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut candidates = vec![build.to_string()];
    if let Some(stripped) = build.strip_suffix("-RELEASE") {
        candidates.push(stripped.to_string());
    }
    if let Some(stripped) = build.strip_prefix("cdda-") {
        candidates.push(stripped.to_string());
    }
    candidates.sort();
    candidates.dedup();

    for candidate in candidates {
        let root = build_dir(game_root, &candidate).join("lang").join("po");
        for path in [
            root.join(format!("{language}.po")),
            root.join(language)
                .join("LC_MESSAGES")
                .join("cataclysm-dda.po"),
            root.join(language).join("cataclysm-dda.po"),
        ] {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

fn parse_po_translations(text: &str) -> HashMap<String, String> {
    let mut translations = HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_value: Option<String> = None;
    let mut active = PoField::None;

    for line in text.lines().chain(std::iter::once("")) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if let (Some(key), Some(value)) = (current_key.take(), current_value.take())
                && !key.is_empty()
                && !value.is_empty()
            {
                translations.insert(key, value);
            }
            active = PoField::None;
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("msgid ") {
            current_key = po_quoted(rest);
            current_value = None;
            active = PoField::MsgId;
        } else if let Some(rest) = trimmed.strip_prefix("msgstr ") {
            current_value = po_quoted(rest);
            active = PoField::MsgStr;
        } else if trimmed.starts_with("msgctxt ") || trimmed.starts_with("msgid_plural ") {
            active = PoField::None;
        } else if let Some(extra) = po_quoted(trimmed) {
            match active {
                PoField::MsgId => {
                    if let Some(key) = current_key.as_mut() {
                        key.push_str(&extra);
                    }
                }
                PoField::MsgStr => {
                    if let Some(value) = current_value.as_mut() {
                        value.push_str(&extra);
                    }
                }
                PoField::None => {}
            }
        }
    }

    translations
}

#[derive(Debug, Clone, Copy)]
enum PoField {
    None,
    MsgId,
    MsgStr,
}

fn po_quoted(value: &str) -> Option<String> {
    let value = value.trim();
    if !value.starts_with('"') || !value.ends_with('"') {
        return None;
    }
    serde_json::from_str::<String>(value).ok()
}

fn fetch_cached(url: &str, cache_path: &Path) -> Result<String, String> {
    if cache_path.is_file() {
        return fs::read_to_string(cache_path)
            .map_err(|error| format!("Failed to read cache {}: {error}", cache_path.display()));
    }

    let client = http::client("guide")?;
    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("Guide request failed: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("Guide request returned HTTP {}", response.status()));
    }

    let text = response
        .text()
        .map_err(|error| format!("Failed to read guide response: {error}"))?;
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create guide cache: {error}"))?;
    }
    fs::write(cache_path, &text).map_err(|error| {
        format!(
            "Failed to write guide cache {}: {error}",
            cache_path.display()
        )
    })?;
    Ok(text)
}

fn collect_entries(
    objects: &[Map<String, Value>],
    translations: &HashMap<String, String>,
    seen: &mut HashSet<String>,
    entries: &mut Vec<GuideSearchResult>,
) {
    for map in objects {
        if let Some(result) = object_to_result(map, translations) {
            let key = format!("{}:{}", result.kind, result.id);
            if seen.insert(key) {
                entries.push(result);
            }
        }
    }
}

fn object_id_index<'a>(
    objects: &[&'a Map<String, Value>],
    _translations: &HashMap<String, String>,
) -> HashMap<String, &'a Map<String, Value>> {
    let mut index = HashMap::new();
    for map in objects {
        if let Some(id) = raw_field_text(map, "id") {
            index.entry(id).or_insert(*map);
        }
    }
    index
}

fn resolve_copy_from(
    map: &Map<String, Value>,
    object_index: &HashMap<String, &Map<String, Value>>,
    translations: &HashMap<String, String>,
    depth: usize,
) -> Map<String, Value> {
    if depth > 16 {
        return map.clone();
    }
    let mut resolved = map
        .get("copy-from")
        .and_then(raw_value_text)
        .and_then(|parent_id| object_index.get(&parent_id).copied())
        .map(|parent| resolve_copy_from(parent, object_index, translations, depth + 1))
        .unwrap_or_default();
    apply_delete_modifier(&mut resolved, map.get("delete"));
    apply_extend_modifier(&mut resolved, map.get("extend"));
    apply_relative_modifier(&mut resolved, map.get("relative"));
    for (key, value) in map {
        resolved.insert(key.clone(), value.clone());
    }
    resolved
}

fn apply_delete_modifier(resolved: &mut Map<String, Value>, modifier: Option<&Value>) {
    let Some(Value::Object(delete)) = modifier else {
        return;
    };
    for (key, value) in delete {
        if let Some(target) = resolved.get_mut(key) {
            delete_value(target, value);
        }
    }
}

fn delete_value(target: &mut Value, delete: &Value) {
    match target {
        Value::Array(values) => {
            values.retain(|value| !modifier_contains_value(delete, value));
        }
        Value::Object(map) => {
            if let Value::Object(delete_map) = delete {
                for key in delete_map.keys() {
                    map.remove(key);
                }
            }
        }
        _ => {}
    }
}

fn modifier_contains_value(modifier: &Value, candidate: &Value) -> bool {
    match modifier {
        Value::Array(values) => values
            .iter()
            .any(|value| modifier_contains_value(value, candidate)),
        other => other == candidate,
    }
}

fn apply_extend_modifier(resolved: &mut Map<String, Value>, modifier: Option<&Value>) {
    let Some(Value::Object(extend)) = modifier else {
        return;
    };
    for (key, value) in extend {
        match resolved.get_mut(key) {
            Some(target) => extend_value(target, value),
            None => {
                resolved.insert(key.clone(), value.clone());
            }
        }
    }
}

fn extend_value(target: &mut Value, extend: &Value) {
    match (target, extend) {
        (Value::Array(target), Value::Array(values)) => {
            target.extend(values.iter().cloned());
        }
        (Value::Array(target), value) => target.push(value.clone()),
        (Value::Object(target), Value::Object(values)) => {
            target.extend(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        (target, value) => *target = value.clone(),
    }
}

fn apply_relative_modifier(resolved: &mut Map<String, Value>, modifier: Option<&Value>) {
    let Some(Value::Object(relative)) = modifier else {
        return;
    };
    for (key, value) in relative {
        if let Some(target) = resolved.get_mut(key) {
            apply_relative_value(target, value);
        }
    }
}

fn apply_relative_value(target: &mut Value, relative: &Value) {
    match (target, relative) {
        (Value::Number(target), Value::Number(relative)) => {
            let Some(sum) = target
                .as_f64()
                .zip(relative.as_f64())
                .and_then(|(target, relative)| serde_json::Number::from_f64(target + relative))
            else {
                return;
            };
            *target = sum;
        }
        (Value::String(target), Value::String(relative)) => {
            if let Some(updated) = apply_relative_string(target, relative) {
                *target = updated;
            }
        }
        _ => {}
    }
}

fn apply_relative_string(target: &str, relative: &str) -> Option<String> {
    let (target_value, target_unit) = parse_quantity(target)?;
    if let Some(percent) = parse_percent(relative) {
        return Some(format_quantity(
            target_value * (1.0 + percent / 100.0),
            target_unit,
        ));
    }
    let (relative_value, relative_unit) = parse_quantity(relative)?;
    if target_unit != relative_unit {
        return None;
    }
    Some(format_quantity(target_value + relative_value, target_unit))
}

fn parse_percent(value: &str) -> Option<f64> {
    value.trim().strip_suffix('%')?.trim().parse::<f64>().ok()
}

fn parse_quantity(value: &str) -> Option<(f64, &str)> {
    let value = value.trim();
    let mut end = 0;
    for (index, ch) in value.char_indices() {
        if ch.is_ascii_digit() || matches!(ch, '+' | '-' | '.') {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    let number = value[..end].parse::<f64>().ok()?;
    Some((number, value[end..].trim()))
}

fn format_quantity(value: f64, unit: &str) -> String {
    let number = if (value.fract()).abs() < f64::EPSILON {
        format!("{}", value as i64)
    } else {
        let mut text = format!("{value:.3}");
        while text.contains('.') && text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
        text
    };
    if unit.is_empty() {
        number
    } else {
        format!("{number} {unit}")
    }
}

fn object_to_result(
    map: &Map<String, Value>,
    translations: &HashMap<String, String>,
) -> Option<GuideSearchResult> {
    let kind = raw_field_text(map, "type").unwrap_or_else(|| "entry".to_string());
    let id = object_identity_id(map, &kind, translations)?;
    let name = field_text(map, "name", translations).unwrap_or_else(|| id.clone());
    let description = field_text(map, "description", translations).unwrap_or_default();
    let mut fields = Vec::new();

    const PRIMARY_FIELDS: &[&str] = &[
        "abstract",
        "copy-from",
        "_source_file",
        "looks_like",
        "category",
        "subcategory",
        "proportional",
        "snippet_category",
        "symbol",
        "color",
        "volume",
        "weight",
        "longest_side",
        "price",
        "price_postapoc",
        "count",
        "charges",
        "stack_size",
        "material",
        "flags",
        "qualities",
        "techniques",
        "use_action",
        "ammo",
        "ammo_effects",
        "magazine_well",
        "range",
        "dispersion",
        "recoil",
        "damage",
        "melee_damage",
        "to_hit",
        "attack_cost",
        "bashing",
        "cutting",
        "qualities",
        "calories",
        "quench",
        "healthy",
        "vitamins",
        "comestible_type",
        "hp",
        "speed",
        "aggression",
        "morale",
        "melee_skill",
        "melee_dice",
        "melee_dice_sides",
        "armor_bash",
        "armor_cut",
        "armor_bullet",
        "armor_acid",
        "armor_fire",
        "species",
        "biosignature",
        "difficulty",
        "skills",
        "proficiencies",
        "components",
        "result",
        "byproducts",
        "tools",
        "using",
        "time",
        "qualities",
        "tiles",
        "tileset",
        "fg",
        "bg",
        "sprite",
        "multitile",
        "additional_tiles",
        "fallback",
        "harvest",
        "death_function",
    ];
    for key in PRIMARY_FIELDS {
        add_compact_field(&mut fields, map, key, translations);
    }
    for summary in use_action_summaries(map.get("use_action"), translations) {
        fields.push(("use_action_summary".to_string(), summary));
    }
    for summary in pocket_summaries(map.get("pocket_data"), translations) {
        fields.push(("pocket_summary".to_string(), summary));
    }
    for summary in quality_summaries(map.get("qualities"), translations) {
        fields.push(("quality_summary".to_string(), summary));
    }
    for summary in object_section_summaries(
        map.get("armor_data"),
        translations,
        "armor",
        &[
            ("encumbrance", "enc"),
            ("coverage", "coverage"),
            ("covers", "covers"),
            ("material_thickness", "thickness"),
            ("env_protec", "env"),
            ("warmth", "warmth"),
            ("storage", "storage"),
        ],
    ) {
        fields.push(("armor_summary".to_string(), summary));
    }
    for summary in object_section_summaries(
        map.get("gun_data"),
        translations,
        "gun",
        &[
            ("ammo", "ammo"),
            ("skill", "skill"),
            ("range", "range"),
            ("ranged_damage", "damage"),
            ("dispersion", "dispersion"),
            ("durability", "durability"),
            ("min_cycle_recoil", "cycle recoil"),
            ("modes", "modes"),
        ],
    ) {
        fields.push(("gun_summary".to_string(), summary));
    }
    if let Some(summary) = object_section_summary(
        &Value::Object(map.clone()),
        translations,
        "melee",
        &[
            ("melee_damage", "damage"),
            ("to_hit", "to hit"),
            ("attack_cost", "attack cost"),
            ("bashing", "bash"),
            ("cutting", "cut"),
            ("techniques", "techniques"),
        ],
    ) {
        if summary != "melee" && summary.contains(":") {
            fields.push(("melee_summary".to_string(), summary));
        }
    }
    for summary in object_section_summaries(
        map.get("tool_data"),
        translations,
        "tool",
        &[
            ("ammo", "ammo"),
            ("max_charges", "max charges"),
            ("initial_charges", "initial charges"),
            ("charges_per_use", "charges/use"),
            ("turns_per_charge", "turns/charge"),
            ("revert_to", "reverts to"),
            ("subtype", "subtype"),
        ],
    ) {
        fields.push(("tool_summary".to_string(), summary));
    }
    for summary in object_section_summaries(
        map.get("magazine_data"),
        translations,
        "magazine",
        &[
            ("ammo_type", "ammo"),
            ("capacity", "capacity"),
            ("reload_time", "reload time"),
            ("linkage", "linkage"),
            ("count", "count"),
        ],
    ) {
        fields.push(("magazine_summary".to_string(), summary));
    }
    for summary in object_section_summaries(
        map.get("book_data"),
        translations,
        "book",
        &[
            ("skill", "skill"),
            ("required_level", "required"),
            ("max_level", "max"),
            ("intelligence", "int"),
            ("time", "time"),
            ("fun", "fun"),
            ("chapters", "chapters"),
        ],
    ) {
        fields.push(("book_summary".to_string(), summary));
    }
    if let Some(summary) = object_section_summary(
        &Value::Object(map.clone()),
        translations,
        "comestible",
        &[
            ("comestible_type", "type"),
            ("calories", "calories"),
            ("quench", "quench"),
            ("healthy", "healthy"),
            ("fun", "fun"),
            ("spoils_in", "spoils in"),
            ("addiction_type", "addiction"),
            ("vitamins", "vitamins"),
        ],
    ) {
        if summary != "comestible" && summary.contains(":") {
            fields.push(("comestible_summary".to_string(), summary));
        }
    }
    for summary in object_section_summaries(
        map.get("seed_data"),
        translations,
        "seed",
        &[
            ("plant_name", "plant"),
            ("fruit", "fruit"),
            ("byproducts", "byproducts"),
            ("grow", "grow"),
            ("fruit_div", "fruit div"),
            ("required_terrain", "terrain"),
        ],
    ) {
        fields.push(("seed_summary".to_string(), summary));
    }
    let mut extra_keys = map.keys().map(String::as_str).collect::<Vec<_>>();
    extra_keys.sort_unstable();
    for key in extra_keys {
        if !is_identity_field(key) && !PRIMARY_FIELDS.contains(&key) {
            add_compact_field(&mut fields, map, key, translations);
        }
    }

    Some(GuideSearchResult {
        id,
        kind,
        name,
        description,
        fields,
        raw_json: serde_json::to_string_pretty(&Value::Object(map.clone())).unwrap_or_default(),
    })
}

fn object_identity_id(
    map: &Map<String, Value>,
    kind: &str,
    _translations: &HashMap<String, String>,
) -> Option<String> {
    raw_field_text(map, "id").or_else(|| match kind {
        "recipe" | "uncraft" => {
            raw_field_text(map, "result").map(|result| format!("{kind}/{result}"))
        }
        "monstergroup" => raw_field_text(map, "name"),
        _ => None,
    })
}

fn is_identity_field(key: &str) -> bool {
    matches!(key, "id" | "type" | "name" | "description")
}

fn add_compact_field(
    fields: &mut Vec<(String, String)>,
    map: &Map<String, Value>,
    key: &str,
    translations: &HashMap<String, String>,
) {
    if let Some(value) = map
        .get(key)
        .and_then(|value| compact_value(value, translations))
    {
        fields.push((key.to_string(), value));
    }
}

fn use_action_summaries(
    value: Option<&Value>,
    translations: &HashMap<String, String>,
) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| use_action_summary(value, translations))
            .collect(),
        Value::Object(_) | Value::String(_) => use_action_summary(value, translations)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn use_action_summary(value: &Value, translations: &HashMap<String, String>) -> Option<String> {
    let Value::Object(map) = value else {
        return compact_value(value, translations).map(|value| format!("action: {value}"));
    };
    let mut parts = Vec::new();
    let label = map
        .get("type")
        .or_else(|| map.get("action"))
        .and_then(|value| compact_value(value, translations))
        .unwrap_or_else(|| "action".to_string());
    parts.push(label);

    for (key, label) in [
        ("target", "target"),
        ("menu_text", "menu"),
        ("msg", "msg"),
        ("active", "active"),
        ("need_charges", "needs charges"),
        ("charges_to_use", "charges/use"),
        ("moves", "moves"),
        ("transform_age", "age"),
        ("not_ready_msg", "not ready"),
    ] {
        if let Some(value) = field_text(map, key, translations) {
            parts.push(format!("{label}: {value}"));
        }
    }

    let summary = parts.join("; ");
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

fn pocket_summaries(value: Option<&Value>, translations: &HashMap<String, String>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| pocket_summary(value, translations))
            .collect(),
        Value::Object(_) => pocket_summary(value, translations).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn pocket_summary(value: &Value, translations: &HashMap<String, String>) -> Option<String> {
    let Value::Object(map) = value else {
        return compact_value(value, translations);
    };
    let mut parts = Vec::new();
    let pocket_type =
        field_text(map, "pocket_type", translations).unwrap_or_else(|| "pocket".into());
    parts.push(pocket_type);

    for (key, label) in [
        ("max_contains_volume", "volume"),
        ("max_contains_weight", "weight"),
        ("max_item_length", "length"),
        ("moves", "moves"),
        ("rigid", "rigid"),
        ("holster", "holster"),
        ("watertight", "watertight"),
        ("airtight", "airtight"),
    ] {
        if let Some(value) = field_text(map, key, translations) {
            parts.push(format!("{label}: {value}"));
        }
    }

    let ammo = extract_ammo_restrictions(value);
    if !ammo.is_empty() {
        parts.push(format!("ammo: {}", ammo.join(", ")));
    }

    if let Some(value) = map
        .get("sealed_data")
        .and_then(|value| compact_value(value, translations))
    {
        parts.push(format!("sealed: {value}"));
    }

    let summary = parts.join("; ");
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

fn quality_summaries(value: Option<&Value>, translations: &HashMap<String, String>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| quality_summary(value, translations))
            .collect(),
        _ => quality_summary(value, translations).into_iter().collect(),
    }
}

fn quality_summary(value: &Value, translations: &HashMap<String, String>) -> Option<String> {
    match value {
        Value::Array(values) if values.len() >= 2 => {
            let quality = compact_value(&values[0], translations)?;
            let level = compact_value(&values[1], translations)?;
            Some(format!("{quality}: level {level}"))
        }
        Value::Object(map) => {
            let quality = map
                .get("id")
                .or_else(|| map.get("quality"))
                .and_then(|value| compact_value(value, translations))?;
            let level = map
                .get("level")
                .and_then(|value| compact_value(value, translations))
                .unwrap_or_else(|| "1".to_string());
            Some(format!("{quality}: level {level}"))
        }
        _ => compact_value(value, translations),
    }
}

fn object_section_summaries(
    value: Option<&Value>,
    translations: &HashMap<String, String>,
    fallback_label: &str,
    keys: &[(&str, &str)],
) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| object_section_summary(value, translations, fallback_label, keys))
            .collect(),
        Value::Object(_) => object_section_summary(value, translations, fallback_label, keys)
            .into_iter()
            .collect(),
        _ => compact_value(value, translations).into_iter().collect(),
    }
}

fn object_section_summary(
    value: &Value,
    translations: &HashMap<String, String>,
    fallback_label: &str,
    keys: &[(&str, &str)],
) -> Option<String> {
    let Value::Object(map) = value else {
        return compact_value(value, translations);
    };
    let mut parts = Vec::new();
    let label = map
        .get("material")
        .or_else(|| map.get("id"))
        .or_else(|| map.get("type"))
        .and_then(|value| compact_value(value, translations))
        .unwrap_or_else(|| fallback_label.to_string());
    parts.push(label);

    for (key, label) in keys {
        if let Some(value) = field_text(map, key, translations) {
            parts.push(format!("{label}: {value}"));
        }
    }

    let summary = parts.join("; ");
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

fn add_derived_fields(
    objects: &[Map<String, Value>],
    entries: &mut [GuideSearchResult],
    translations: &HashMap<String, String>,
) {
    let mut index = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        index.insert(entry.id.clone(), idx);
    }

    let group_index = item_group_index(objects, translations);
    let requirement_index = requirement_index(objects, translations);
    let quality_index = quality_index(objects, translations);
    let harvest_sources = harvest_source_index(objects, translations);
    add_definition_summary_fields(objects, entries, &index, translations);
    for map in objects {
        let kind = raw_field_text(map, "type").unwrap_or_default();
        match kind.as_str() {
            "recipe" => add_recipe_fields(
                map,
                entries,
                &index,
                &requirement_index,
                &quality_index,
                translations,
                false,
            ),
            "uncraft" => add_recipe_fields(
                map,
                entries,
                &index,
                &requirement_index,
                &quality_index,
                translations,
                true,
            ),
            "item_group" => add_item_group_fields(map, entries, &index, &group_index, translations),
            "MONSTER" => add_monster_fields(map, entries, &index, translations),
            "monstergroup" => add_monster_group_fields(map, entries, &index, translations),
            "construction" => add_construction_fields(
                map,
                entries,
                &index,
                &requirement_index,
                &quality_index,
                translations,
            ),
            "vehicle_part" => add_vehicle_part_fields(map, entries, &index, translations),
            "harvest" => add_harvest_fields(map, entries, &index, &harvest_sources, translations),
            _ => {}
        }
        add_ammo_magazine_fields(map, entries, &index, translations);
    }
    add_cross_reference_fields(objects, entries, &index, translations);
}

fn add_definition_summary_fields(
    objects: &[Map<String, Value>],
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    translations: &HashMap<String, String>,
) {
    let flag_definitions = definition_index(objects, translations, "json_flag");
    let technique_definitions = definition_index(objects, translations, "technique");
    let material_definitions = material_definition_index(objects, translations);
    let skill_definitions = definition_index(objects, translations, "skill");
    let proficiency_definitions = definition_index(objects, translations, "proficiency");
    let vitamin_definitions = definition_index(objects, translations, "vitamin");
    for map in objects {
        let kind = raw_field_text(map, "type").unwrap_or_default();
        let Some(source_id) = object_identity_id(map, &kind, translations) else {
            continue;
        };
        for flag in map
            .get("flags")
            .map(extract_string_tokens)
            .unwrap_or_default()
        {
            if let Some(summary) = flag_definitions.get(&flag) {
                push_relation(
                    entries,
                    index,
                    &source_id,
                    "flag_summary",
                    &format!("{flag}: {summary}"),
                );
            }
        }
        for material in material_tokens(map) {
            if let Some(summary) = material_definitions.get(&material) {
                push_relation(
                    entries,
                    index,
                    &source_id,
                    "material_summary",
                    &format!("{material}: {summary}"),
                );
            }
        }
        for skill in definition_field_tokens(map, &["skill", "skills", "melee_skill"]) {
            if let Some(summary) = skill_definitions.get(&skill) {
                push_relation(
                    entries,
                    index,
                    &source_id,
                    "skill_summary",
                    &format!("{skill}: {summary}"),
                );
            }
        }
        for proficiency in definition_field_tokens(map, &["proficiency", "proficiencies"]) {
            if let Some(summary) = proficiency_definitions.get(&proficiency) {
                push_relation(
                    entries,
                    index,
                    &source_id,
                    "proficiency_summary",
                    &format!("{proficiency}: {summary}"),
                );
            }
        }
        for vitamin in definition_field_tokens(map, &["vitamins"]) {
            if let Some(summary) = vitamin_definitions.get(&vitamin) {
                push_relation(
                    entries,
                    index,
                    &source_id,
                    "vitamin_summary",
                    &format!("{vitamin}: {summary}"),
                );
            }
        }
        for technique in map
            .get("techniques")
            .map(extract_string_tokens)
            .unwrap_or_default()
        {
            if let Some(summary) = technique_definitions.get(&technique) {
                push_relation(
                    entries,
                    index,
                    &source_id,
                    "technique_summary",
                    &format!("{technique}: {summary}"),
                );
            }
        }
    }
}

fn definition_index(
    objects: &[Map<String, Value>],
    translations: &HashMap<String, String>,
    expected_kind: &str,
) -> HashMap<String, String> {
    let mut definitions = HashMap::new();
    for map in objects {
        let kind = raw_field_text(map, "type").unwrap_or_default();
        if kind != expected_kind {
            continue;
        }
        let Some(id) = raw_field_text(map, "id") else {
            continue;
        };
        if let Some(summary) = definition_summary(map, &id, translations) {
            definitions.insert(id, summary);
        }
    }
    definitions
}

fn material_definition_index(
    objects: &[Map<String, Value>],
    translations: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut definitions = HashMap::new();
    for map in objects {
        let kind = raw_field_text(map, "type").unwrap_or_default();
        if kind != "material" {
            continue;
        }
        let Some(id) = raw_field_text(map, "id") else {
            continue;
        };
        if let Some(summary) = material_definition_summary(map, &id, translations) {
            definitions.insert(id, summary);
        }
    }
    definitions
}

fn material_definition_summary(
    map: &Map<String, Value>,
    id: &str,
    translations: &HashMap<String, String>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(name) = map
        .get("name")
        .and_then(|value| compact_value(value, translations))
        .filter(|name| name != id)
    {
        parts.push(name);
    }
    for (key, label) in [
        ("density", "density"),
        ("specific_heat_liquid", "specific heat"),
        ("specific_heat_solid", "specific heat solid"),
        ("latent_heat", "latent heat"),
        ("bash_resist", "bash resist"),
        ("cut_resist", "cut resist"),
        ("acid_resist", "acid resist"),
        ("elec_resist", "elec resist"),
        ("fire_resist", "fire resist"),
        ("chip_resist", "chip resist"),
        ("bash_dmg_verb", "bash verb"),
        ("cut_dmg_verb", "cut verb"),
        ("dmg_adj", "damage adj"),
        ("burn_data", "burn"),
        ("repaired_with", "repaired with"),
        ("edible", "edible"),
        ("rotting", "rotting"),
    ] {
        if let Some(value) = field_text(map, key, translations) {
            parts.push(format!("{label}: {value}"));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

fn material_tokens(map: &Map<String, Value>) -> Vec<String> {
    let mut tokens = Vec::new();
    collect_material_tokens(&Value::Object(map.clone()), &mut tokens);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn collect_material_tokens(value: &Value, tokens: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "material" {
                    tokens.extend(extract_string_tokens(child));
                }
                collect_material_tokens(child, tokens);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_material_tokens(child, tokens);
            }
        }
        _ => {}
    }
}

fn definition_field_tokens(map: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    let mut tokens = Vec::new();
    collect_definition_field_tokens(&Value::Object(map.clone()), keys, &mut tokens);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn collect_definition_field_tokens(value: &Value, keys: &[&str], tokens: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if keys.iter().any(|candidate| candidate == key) {
                    tokens.extend(extract_string_tokens(child));
                }
                collect_definition_field_tokens(child, keys, tokens);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_definition_field_tokens(child, keys, tokens);
            }
        }
        _ => {}
    }
}

fn definition_summary(
    map: &Map<String, Value>,
    id: &str,
    translations: &HashMap<String, String>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(name) = map
        .get("name")
        .and_then(|value| compact_value(value, translations))
        .filter(|name| name != id)
    {
        parts.push(name);
    }
    for key in ["info", "description"] {
        if let Some(value) = map
            .get(key)
            .and_then(|value| compact_value(value, translations))
        {
            parts.push(value);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

fn item_group_index(
    objects: &[Map<String, Value>],
    translations: &HashMap<String, String>,
) -> HashMap<String, Vec<String>> {
    let mut groups = HashMap::new();
    for map in objects {
        let kind = map
            .get("type")
            .and_then(|value| compact_value(value, translations))
            .unwrap_or_default();
        if kind != "item_group" {
            continue;
        }
        let Some(group_id) = map
            .get("id")
            .and_then(|value| compact_value(value, translations))
        else {
            continue;
        };
        let tokens = map
            .get("items")
            .or_else(|| map.get("entries"))
            .map(extract_string_tokens)
            .unwrap_or_default();
        groups.insert(group_id, tokens);
    }
    groups
}

#[derive(Debug, Default, Clone)]
struct RequirementParts {
    components: Vec<String>,
    tools: Vec<String>,
    using: Vec<String>,
}

fn requirement_index(
    objects: &[Map<String, Value>],
    translations: &HashMap<String, String>,
) -> HashMap<String, RequirementParts> {
    let mut requirements = HashMap::new();
    for map in objects {
        let kind = map
            .get("type")
            .and_then(|value| compact_value(value, translations))
            .unwrap_or_default();
        if kind != "requirement" {
            continue;
        }
        let Some(id) = map
            .get("id")
            .and_then(|value| compact_value(value, translations))
        else {
            continue;
        };
        requirements.insert(id, requirement_parts(map));
    }
    requirements
}

fn requirement_parts(map: &Map<String, Value>) -> RequirementParts {
    RequirementParts {
        components: map
            .get("components")
            .map(extract_string_tokens)
            .unwrap_or_default(),
        tools: map
            .get("tools")
            .map(extract_string_tokens)
            .unwrap_or_default(),
        using: map
            .get("using")
            .map(extract_string_tokens)
            .unwrap_or_default(),
    }
}

fn quality_index(
    objects: &[Map<String, Value>],
    translations: &HashMap<String, String>,
) -> HashMap<String, Vec<String>> {
    let mut qualities: HashMap<String, Vec<String>> = HashMap::new();
    for map in objects {
        let kind = map
            .get("type")
            .and_then(|value| compact_value(value, translations))
            .unwrap_or_default();
        let Some(item_id) = object_identity_id(map, &kind, translations) else {
            continue;
        };
        for quality in map
            .get("qualities")
            .map(extract_string_tokens)
            .unwrap_or_default()
        {
            let items = qualities.entry(quality).or_default();
            if !items.contains(&item_id) {
                items.push(item_id.clone());
            }
        }
    }
    for items in qualities.values_mut() {
        items.sort();
    }
    qualities
}

fn harvest_source_index(
    objects: &[Map<String, Value>],
    translations: &HashMap<String, String>,
) -> HashMap<String, Vec<String>> {
    let mut sources: HashMap<String, Vec<String>> = HashMap::new();
    for map in objects {
        let kind = map
            .get("type")
            .and_then(|value| compact_value(value, translations))
            .unwrap_or_default();
        if kind != "MONSTER" {
            continue;
        }
        let Some(monster_id) = map
            .get("id")
            .and_then(|value| compact_value(value, translations))
        else {
            continue;
        };
        let Some(harvest_id) = map
            .get("harvest")
            .and_then(|value| compact_value(value, translations))
        else {
            continue;
        };
        let monsters = sources.entry(harvest_id).or_default();
        if !monsters.contains(&monster_id) {
            monsters.push(monster_id);
        }
    }
    for monsters in sources.values_mut() {
        monsters.sort();
    }
    sources
}

fn expand_requirement_parts(
    requirement_id: &str,
    requirement_index: &HashMap<String, RequirementParts>,
    seen: &mut HashSet<String>,
    depth: usize,
) -> RequirementParts {
    if depth > 16 || !seen.insert(requirement_id.to_string()) {
        return RequirementParts::default();
    }
    let Some(requirement) = requirement_index.get(requirement_id) else {
        seen.remove(requirement_id);
        return RequirementParts::default();
    };
    let mut expanded = requirement.clone();
    for nested_id in &requirement.using {
        let nested = expand_requirement_parts(nested_id, requirement_index, seen, depth + 1);
        expanded.components.extend(nested.components);
        expanded.tools.extend(nested.tools);
    }
    expanded.components.sort();
    expanded.components.dedup();
    expanded.tools.sort();
    expanded.tools.dedup();
    seen.remove(requirement_id);
    expanded
}

fn add_cross_reference_fields(
    objects: &[Map<String, Value>],
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    translations: &HashMap<String, String>,
) {
    let mut seen = HashSet::new();
    for map in objects {
        let kind = map
            .get("type")
            .and_then(|value| compact_value(value, translations))
            .unwrap_or_default();
        if matches!(
            kind.as_str(),
            "recipe" | "uncraft" | "item_group" | "MONSTER" | "monstergroup"
        ) {
            continue;
        }
        let Some(source_id) = map
            .get("id")
            .or_else(|| map.get("name"))
            .and_then(|value| compact_value(value, translations))
        else {
            continue;
        };
        let source_label = if kind.is_empty() {
            source_id.clone()
        } else {
            format!("{kind}:{source_id}")
        };
        for token in extract_string_tokens(&Value::Object(map.clone())) {
            if token == source_id {
                continue;
            }
            let Some(target_index) = index.get(&token).copied() else {
                continue;
            };
            if !seen.insert((token.clone(), source_label.clone())) {
                continue;
            }
            if let Some(target) = entries.get_mut(target_index) {
                target
                    .fields
                    .push(("referenced_by".to_string(), source_label.clone()));
            }
            if let Some(relation_key) = source_reference_relation_key(&kind) {
                push_relation(entries, index, &token, relation_key, &source_label);
            }
        }
    }
}

fn source_reference_relation_key(kind: &str) -> Option<&'static str> {
    match kind {
        "mapgen" | "mapgen_palette" => Some("placed_by_mapgen"),
        "map_extra" => Some("placed_by_map_extra"),
        "overmap_special" => Some("placed_by_overmap_special"),
        "effect_on_condition" => Some("referenced_by_eoc"),
        _ => None,
    }
}

fn add_monster_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    translations: &HashMap<String, String>,
) {
    let Some(monster_id) = map
        .get("id")
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };

    for key in ["death_drops", "harvest"] {
        let Some(target_id) = map
            .get(key)
            .and_then(|value| compact_value(value, translations))
        else {
            continue;
        };
        if let Some(target) = index.get(&target_id).and_then(|idx| entries.get_mut(*idx)) {
            target.fields.push((
                "monster_source".to_string(),
                format!("{monster_id} via {key}"),
            ));
        }
    }
}

fn add_monster_group_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    translations: &HashMap<String, String>,
) {
    let Some(group_id) = map
        .get("name")
        .or_else(|| map.get("id"))
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let monsters = map
        .get("monsters")
        .or_else(|| map.get("entries"))
        .map(extract_string_tokens)
        .unwrap_or_default();
    for monster in monsters
        .iter()
        .filter(|monster| index.contains_key(*monster))
    {
        if let Some(target) = index.get(monster).and_then(|idx| entries.get_mut(*idx)) {
            target
                .fields
                .push(("monster_group".to_string(), group_id.clone()));
        }
    }
}

fn add_ammo_magazine_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    translations: &HashMap<String, String>,
) {
    let Some(source_id) = object_identity_id(
        map,
        &map.get("type")
            .and_then(|value| compact_value(value, translations))
            .unwrap_or_default(),
        translations,
    ) else {
        return;
    };

    for ammo in map
        .get("ammo")
        .map(extract_string_tokens)
        .unwrap_or_default()
        .iter()
        .filter(|ammo| index.contains_key(*ammo))
    {
        push_relation(entries, index, ammo, "ammo_used_by", &source_id);
    }

    for magazine in map
        .get("magazines")
        .or_else(|| map.get("magazine"))
        .or_else(|| map.get("magazine_well"))
        .map(extract_string_tokens)
        .unwrap_or_default()
        .iter()
        .filter(|magazine| index.contains_key(*magazine))
    {
        push_relation(entries, index, magazine, "magazine_for", &source_id);
    }

    for ammo in map
        .get("pocket_data")
        .map(extract_ammo_restrictions)
        .unwrap_or_default()
        .iter()
        .filter(|ammo| index.contains_key(*ammo))
    {
        push_relation(entries, index, ammo, "ammo_contained_by", &source_id);
        push_relation(entries, index, &source_id, "contains_ammo", ammo);
    }
}

fn add_construction_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    requirement_index: &HashMap<String, RequirementParts>,
    quality_index: &HashMap<String, Vec<String>>,
    translations: &HashMap<String, String>,
) {
    let Some(construction_id) = map
        .get("id")
        .or_else(|| map.get("group"))
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let label = construction_summary(map, &construction_id, translations);
    let mut components = map
        .get("components")
        .map(extract_string_tokens)
        .unwrap_or_default();
    let mut tools = map
        .get("tools")
        .map(extract_string_tokens)
        .unwrap_or_default();
    for requirement_id in map
        .get("using")
        .map(extract_string_tokens)
        .unwrap_or_default()
    {
        let mut seen = HashSet::new();
        let requirement =
            expand_requirement_parts(&requirement_id, requirement_index, &mut seen, 0);
        components.extend(requirement.components);
        tools.extend(requirement.tools);
    }
    components.sort();
    components.dedup();
    tools.sort();
    tools.dedup();

    for component in components.iter().filter(|item| index.contains_key(*item)) {
        push_relation(entries, index, component, "used_in_construction", &label);
    }
    for tool in tools.iter().filter(|item| index.contains_key(*item)) {
        push_relation(entries, index, tool, "tool_for_construction", &label);
    }
    for quality in tools.iter().filter(|quality| !index.contains_key(*quality)) {
        for item_id in quality_index.get(quality).into_iter().flatten() {
            push_relation(
                entries,
                index,
                item_id,
                "tool_for_construction",
                &format!("{label}; quality: {quality}"),
            );
        }
    }
}

fn construction_summary(
    map: &Map<String, Value>,
    construction_id: &str,
    translations: &HashMap<String, String>,
) -> String {
    let mut parts = vec![format!("construction:{construction_id}")];
    for (key, label) in [
        ("pre_terrain", "from"),
        ("post_terrain", "to"),
        ("time", "time"),
        ("difficulty", "difficulty"),
    ] {
        if let Some(value) = field_text(map, key, translations) {
            parts.push(format!("{label}: {value}"));
        }
    }
    parts.join("; ")
}

fn add_vehicle_part_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    translations: &HashMap<String, String>,
) {
    let Some(part_id) = map
        .get("id")
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let Some(item_id) = map
        .get("item")
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let label = vehicle_part_summary(map, &part_id, translations);
    push_relation(
        entries,
        index,
        &item_id,
        "installed_as_vehicle_part",
        &label,
    );
}

fn vehicle_part_summary(
    map: &Map<String, Value>,
    part_id: &str,
    translations: &HashMap<String, String>,
) -> String {
    let mut parts = vec![format!("vehicle_part:{part_id}")];
    for (key, label) in [
        ("location", "location"),
        ("durability", "durability"),
        ("damage_modifier", "damage mod"),
        ("folded_volume", "folded volume"),
        ("flags", "flags"),
    ] {
        if let Some(value) = field_text(map, key, translations) {
            parts.push(format!("{label}: {value}"));
        }
    }
    parts.join("; ")
}

fn add_harvest_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    harvest_sources: &HashMap<String, Vec<String>>,
    translations: &HashMap<String, String>,
) {
    let Some(harvest_id) = map
        .get("id")
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let base_label = harvest_summary(map, &harvest_id, harvest_sources, translations);
    let tokens = map
        .get("entries")
        .or_else(|| map.get("drops"))
        .map(extract_string_tokens)
        .unwrap_or_default();
    for token in tokens.iter().filter(|token| index.contains_key(*token)) {
        push_relation(entries, index, token, "harvested_from", &base_label);
    }
}

fn harvest_summary(
    map: &Map<String, Value>,
    harvest_id: &str,
    harvest_sources: &HashMap<String, Vec<String>>,
    translations: &HashMap<String, String>,
) -> String {
    let mut parts = vec![format!("harvest:{harvest_id}")];
    if let Some(monsters) = harvest_sources.get(harvest_id) {
        parts.push(format!("monsters: {}", monsters.join(", ")));
    }
    for (key, label) in [("message", "message"), ("leftovers", "leftovers")] {
        if let Some(value) = field_text(map, key, translations) {
            parts.push(format!("{label}: {value}"));
        }
    }
    parts.join("; ")
}

fn add_item_group_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    group_index: &HashMap<String, Vec<String>>,
    translations: &HashMap<String, String>,
) {
    let Some(group_id) = map
        .get("id")
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let subtype = map
        .get("subtype")
        .and_then(|value| compact_value(value, translations))
        .unwrap_or_default();
    let base_label = if subtype.is_empty() {
        group_id.clone()
    } else {
        format!("{group_id} ({subtype})")
    };
    let mut seen = HashSet::new();
    for (item, path) in expand_item_group_members(&group_id, group_index, &mut seen, 0) {
        let Some(target) = index.get(&item).and_then(|idx| entries.get_mut(*idx)) else {
            continue;
        };
        let label = if path.is_empty() {
            base_label.clone()
        } else {
            format!("{base_label} via {}", path.join(" > "))
        };
        if !target
            .fields
            .iter()
            .any(|(key, value)| key == "found_in_group" && value == &label)
        {
            target.fields.push(("found_in_group".to_string(), label));
        }
    }
}

fn expand_item_group_members(
    group_id: &str,
    group_index: &HashMap<String, Vec<String>>,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Vec<(String, Vec<String>)> {
    if depth > 16 || !seen.insert(group_id.to_string()) {
        return Vec::new();
    }
    let mut members = Vec::new();
    for token in group_index.get(group_id).into_iter().flatten() {
        if group_index.contains_key(token) {
            for (item, mut path) in expand_item_group_members(token, group_index, seen, depth + 1) {
                path.insert(0, token.clone());
                members.push((item, path));
            }
        } else {
            members.push((token.clone(), Vec::new()));
        }
    }
    seen.remove(group_id);
    members.sort();
    members.dedup();
    members
}

fn push_relation(
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    target_id: &str,
    key: &str,
    value: &str,
) {
    let Some(target) = index.get(target_id).and_then(|idx| entries.get_mut(*idx)) else {
        return;
    };
    if !target
        .fields
        .iter()
        .any(|(candidate_key, candidate_value)| candidate_key == key && candidate_value == value)
    {
        target.fields.push((key.to_string(), value.to_string()));
    }
}

fn extract_ammo_restrictions(value: &Value) -> Vec<String> {
    let mut tokens = Vec::new();
    collect_ammo_restrictions(value, &mut tokens);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn collect_ammo_restrictions(value: &Value, tokens: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for key in ["ammo_restriction", "ammo_restrictions", "ammo"] {
                if let Some(value) = map.get(key) {
                    tokens.extend(extract_string_tokens(value));
                    collect_object_keys(value, tokens);
                }
            }
            for child in map.values() {
                collect_ammo_restrictions(child, tokens);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_ammo_restrictions(child, tokens);
            }
        }
        _ => {}
    }
}

fn collect_object_keys(value: &Value, tokens: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if looks_like_item_id(key) {
                    tokens.push(key.clone());
                }
                collect_object_keys(child, tokens);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_object_keys(child, tokens);
            }
        }
        _ => {}
    }
}

fn collect_objects<'a>(value: &'a Value, objects: &mut Vec<&'a Map<String, Value>>) {
    match value {
        Value::Object(map) => {
            objects.push(map);
            for child in map.values() {
                collect_objects(child, objects);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_objects(child, objects);
            }
        }
        _ => {}
    }
}

fn add_recipe_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
    requirement_index: &HashMap<String, RequirementParts>,
    quality_index: &HashMap<String, Vec<String>>,
    translations: &HashMap<String, String>,
    uncraft: bool,
) {
    let Some(result) = map
        .get("result")
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let kind = if uncraft { "uncraft" } else { "recipe" };
    let recipe_name = object_identity_id(map, kind, translations).unwrap_or_else(|| result.clone());
    let time = map
        .get("time")
        .and_then(|value| compact_value(value, translations))
        .unwrap_or_default();
    let mut components = map
        .get("components")
        .map(extract_string_tokens)
        .unwrap_or_default();
    let mut tools = map
        .get("tools")
        .map(extract_string_tokens)
        .unwrap_or_default();
    for requirement_id in map
        .get("using")
        .map(extract_string_tokens)
        .unwrap_or_default()
    {
        let mut seen = HashSet::new();
        let requirement =
            expand_requirement_parts(&requirement_id, requirement_index, &mut seen, 0);
        components.extend(requirement.components);
        tools.extend(requirement.tools);
    }
    components.sort();
    components.dedup();
    tools.sort();
    tools.dedup();
    let byproducts = map
        .get("byproducts")
        .map(extract_string_tokens)
        .unwrap_or_default();

    let summary = recipe_summary(&recipe_name, &components, &tools, &byproducts, &time);
    if let Some(target) = index.get(&result).and_then(|idx| entries.get_mut(*idx)) {
        target.fields.push((
            if uncraft {
                "uncraft_from"
            } else {
                "crafted_by"
            }
            .to_string(),
            summary.clone(),
        ));
    }

    for component in components
        .iter()
        .filter(|component| index.contains_key(*component))
    {
        if let Some(target) = index.get(component).and_then(|idx| entries.get_mut(*idx)) {
            target.fields.push((
                if uncraft {
                    "uncraft_uses"
                } else {
                    "used_by_recipe"
                }
                .to_string(),
                format!("{recipe_name} -> {result}"),
            ));
        }
    }

    for tool in tools.iter().filter(|tool| index.contains_key(*tool)) {
        if let Some(target) = index.get(tool).and_then(|idx| entries.get_mut(*idx)) {
            target.fields.push((
                if uncraft {
                    "tool_for_uncraft"
                } else {
                    "tool_for_recipe"
                }
                .to_string(),
                format!("{recipe_name} -> {result}"),
            ));
        }
    }
    for quality in tools.iter().filter(|quality| !index.contains_key(*quality)) {
        for item_id in quality_index.get(quality).into_iter().flatten() {
            push_relation(
                entries,
                index,
                item_id,
                if uncraft {
                    "tool_for_uncraft"
                } else {
                    "tool_for_recipe"
                },
                &format!("{recipe_name} -> {result}; quality: {quality}"),
            );
        }
    }

    for byproduct in byproducts
        .iter()
        .filter(|byproduct| index.contains_key(*byproduct))
    {
        if let Some(target) = index.get(byproduct).and_then(|idx| entries.get_mut(*idx)) {
            target.fields.push((
                if uncraft {
                    "byproduct_of_uncraft"
                } else {
                    "byproduct_of_recipe"
                }
                .to_string(),
                format!("{recipe_name} -> {result}"),
            ));
        }
    }
}

fn recipe_summary(
    recipe_name: &str,
    components: &[String],
    tools: &[String],
    byproducts: &[String],
    time: &str,
) -> String {
    let mut parts = vec![recipe_name.to_string()];
    if !components.is_empty() {
        parts.push(format!("components: {}", components.join(", ")));
    }
    if !tools.is_empty() {
        parts.push(format!("tools: {}", tools.join(", ")));
    }
    if !byproducts.is_empty() {
        parts.push(format!("byproducts: {}", byproducts.join(", ")));
    }
    if !time.is_empty() {
        parts.push(format!("time: {time}"));
    }
    parts.join("; ")
}

fn extract_string_tokens(value: &Value) -> Vec<String> {
    let mut tokens = Vec::new();
    collect_string_tokens(value, &mut tokens);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn collect_string_tokens(value: &Value, tokens: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if looks_like_item_id(text) {
                tokens.push(text.clone());
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_string_tokens(child, tokens);
            }
        }
        Value::Object(map) => {
            for child in map.values() {
                collect_string_tokens(child, tokens);
            }
        }
        _ => {}
    }
}

fn looks_like_item_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
}

fn is_relation_field(key: &str) -> bool {
    matches!(
        key,
        "crafted_by"
            | "used_by_recipe"
            | "uncraft_from"
            | "uncraft_uses"
            | "tool_for_recipe"
            | "tool_for_uncraft"
            | "byproduct_of_recipe"
            | "byproduct_of_uncraft"
            | "ammo_used_by"
            | "magazine_for"
            | "ammo_contained_by"
            | "contains_ammo"
            | "used_in_construction"
            | "tool_for_construction"
            | "installed_as_vehicle_part"
            | "placed_by_mapgen"
            | "placed_by_map_extra"
            | "placed_by_overmap_special"
            | "referenced_by_eoc"
            | "harvested_from"
            | "found_in_group"
            | "monster_source"
            | "monster_group"
            | "referenced_by"
    )
}

fn is_tile_field(key: &str) -> bool {
    matches!(
        key,
        "tile_match" | "tiles" | "tileset" | "fg" | "bg" | "sprite"
    )
}

fn extract_relation_ids(value: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for token in value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/')))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if looks_like_relation_id(token) && !targets.iter().any(|target| target == token) {
            targets.push(token.to_string());
        }
    }
    targets
}

fn looks_like_relation_id(value: &str) -> bool {
    const STOP_WORDS: &[&str] = &[
        "byproducts",
        "collection",
        "components",
        "construction",
        "death_drops",
        "distribution",
        "harvest",
        "item",
        "items",
        "map_extra",
        "mapgen",
        "monster",
        "overmap_special",
        "recipe",
        "result",
        "time",
        "tools",
        "type",
        "using",
        "vehicle_part",
        "via",
    ];
    looks_like_item_id(value)
        && value.len() > 1
        && !value.chars().all(|ch| ch.is_ascii_digit())
        && !STOP_WORDS
            .iter()
            .any(|word| word.eq_ignore_ascii_case(value))
}

fn field_text(
    map: &Map<String, Value>,
    key: &str,
    translations: &HashMap<String, String>,
) -> Option<String> {
    let value = map.get(key)?;
    if key.contains("damage") {
        if let Some(text) = damage_value_text(value, translations) {
            return Some(text);
        }
    }
    compact_value(value, translations)
}

fn raw_field_text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(raw_value_text)
}

fn raw_value_text(value: &Value) -> Option<String> {
    compact_value(value, &HashMap::new())
}

fn damage_value_text(value: &Value, translations: &HashMap<String, String>) -> Option<String> {
    match value {
        Value::Array(values) => {
            let parts = values
                .iter()
                .filter_map(|value| damage_value_text(value, translations))
                .collect::<Vec<_>>();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(", "))
            }
        }
        Value::Object(map) => {
            let damage_type = map
                .get("damage_type")
                .and_then(|value| compact_value(value, translations))?;
            let amount = map
                .get("amount")
                .and_then(|value| compact_value(value, translations))?;
            Some(format!("{damage_type} {amount}"))
        }
        _ => None,
    }
}

fn compact_value(value: &Value, translations: &HashMap<String, String>) -> Option<String> {
    let text = match value {
        Value::Null => return None,
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => translate(value, translations),
        Value::Array(values) => values
            .iter()
            .filter_map(|value| compact_value(value, translations))
            .take(8)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => {
            if let Some(value) = map
                .get("str")
                .and_then(|value| compact_value(value, translations))
            {
                value
            } else if let Some(value) = map
                .get("str_sp")
                .and_then(|value| compact_value(value, translations))
            {
                value
            } else if let Some(value) = map
                .get("str_pl")
                .and_then(|value| compact_value(value, translations))
            {
                value
            } else {
                map.iter()
                    .take(6)
                    .filter_map(|(key, value)| {
                        compact_value(value, translations).map(|value| format!("{key}: {value}"))
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        }
    };

    if text.is_empty() { None } else { Some(text) }
}

fn translate(text: &str, translations: &HashMap<String, String>) -> String {
    translations
        .get(text)
        .cloned()
        .unwrap_or_else(|| text.to_string())
}

pub fn cache_summary(game_root: &Path) -> PathBuf {
    guide_cache_dir(game_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_dataset_falls_back_to_english_when_language_is_missing() {
        let root = std::env::temp_dir().join(format!("cddock-guide-test-{}", std::process::id()));
        let build = "cdda-0.I-2026-03-05-0143";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            format!(r#"[{{"build_number":"{build}","prerelease":true,"langs":[]}}]"#),
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[{"type":"GENERIC","id":"hammer","name":"hammer","flags":["HAMMER"]}]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "zh_CN").expect("dataset");
        assert_eq!(dataset.language(), "en");
        assert!(
            dataset
                .warning()
                .is_some_and(|warning| warning.contains("using English"))
        );
        assert_eq!(search_dataset(&dataset, "HAMMER", 10).len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_uses_local_po_when_guide_language_is_missing() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-local-po-test-{}", std::process::id()));
        let build = "cdda-0.I-2026-03-05-0143";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            format!(r#"[{{"build_number":"{build}","prerelease":true,"langs":[]}}]"#),
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[{"type":"GENERIC","id":"hammer","name":"hammer","description":"A sturdy hammer."}]"#,
        )
        .expect("all cache");
        let po_dir = build_dir(&root, build).join("lang").join("po");
        fs::create_dir_all(&po_dir).expect("po dir");
        fs::write(
            po_dir.join("zh_CN.po"),
            r#"
msgid "hammer"
msgstr "锤子"

msgid ""
"A sturdy "
"hammer."
msgstr "一把结实的锤子。"
"#,
        )
        .expect("po file");

        let dataset = load_dataset(&root, build, "zh_CN").expect("dataset");
        assert_eq!(dataset.language(), "zh_CN");
        assert!(
            dataset
                .warning()
                .is_some_and(|warning| warning.contains("installed game translations"))
        );
        let hammer = dataset.get("hammer").expect("hammer");
        assert_eq!(hammer.name, "锤子");
        assert_eq!(hammer.description, "一把结实的锤子。");
        assert_eq!(search_dataset(&dataset, "锤子", 10).len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_reads_installed_game_json_before_remote_guide() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-local-test-{}", std::process::id()));
        let build = "local-build";
        let data = build_dir(&root, build).join("data").join("json");
        fs::create_dir_all(&data).expect("local data dir");
        let mod_data = build_dir(&root, build)
            .join("data")
            .join("mods")
            .join("test_mod");
        fs::create_dir_all(&mod_data).expect("mod data dir");
        fs::write(
            data.join("items.json"),
            r#"[
                {
                    "type":"GENERIC",
                    "id":"local_pole",
                    "name":"local pole",
                    "flags":["LOCAL_FLAG"],
                    "qualities":[["HAMMER",2]]
                },
                {
                    "type":"json_flag",
                    "id":"LOCAL_FLAG",
                    "info":"Only present in the installed local build."
                }
            ]"#,
        )
        .expect("local json");
        fs::write(
            mod_data.join("items.json"),
            r#"[{"type":"GENERIC","id":"mod_pole","name":"mod pole"}]"#,
        )
        .expect("mod json");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let item = dataset.get("local_pole").expect("local item");
        assert_eq!(item.name, "local pole");
        assert!(item.fields.iter().any(|(key, value)| {
            key == "flag_summary" && value.contains("installed local build")
        }));
        assert!(
            item.fields
                .iter()
                .any(|(key, value)| { key == "quality_summary" && value == "HAMMER: level 2" })
        );
        assert_eq!(
            search_dataset(&dataset, "flag_summary installed", 10).len(),
            1
        );
        assert!(item.fields.iter().any(|(key, value)| {
            key == "_source_file" && value.ends_with("data/json/items.json")
        }));
        assert_eq!(dataset.get("mod_pole").expect("mod item").name, "mod pole");
        assert!(
            dataset
                .get("mod_pole")
                .expect("mod item")
                .fields
                .iter()
                .any(|(key, value)| {
                    key == "_source_file" && value.ends_with("data/mods/test_mod/items.json")
                })
        );
        assert_eq!(search_dataset(&dataset, "mod_pole", 10).len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_indexes_unlisted_detail_fields() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-fields-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {
                    "type":"GENERIC",
                    "id":"hiking_pack",
                    "name":"hiking pack",
                    "pocket_data":[
                        {
                            "pocket_type":"CONTAINER",
                            "max_contains_volume":"20 L",
                            "max_contains_weight":"5 kg",
                            "max_item_length":"1 m",
                            "moves":100,
                            "rigid":true
                        },
                        {
                            "pocket_type":"MAGAZINE",
                            "ammo_restriction":{"9mm":15}
                        }
                    ],
                    "relative":{"weight":"80 g"},
                    "delete":{"flags":["OLD_FLAG"]},
                    "extend":{"flags":["NEW_FLAG"]}
                },
                {
                    "type":"ARMOR",
                    "id":"leather_jacket",
                    "name":"leather jacket",
                    "armor_data":[
                        {
                            "material":"leather",
                            "covers":["torso","arm_l","arm_r"],
                            "coverage":95,
                            "encumbrance":12,
                            "material_thickness":2,
                            "env_protec":1,
                            "warmth":20,
                            "storage":"1 L"
                        }
                    ]
                },
                {
                    "type":"GUN",
                    "id":"pistol_9mm",
                    "name":"9mm pistol",
                    "gun_data":{
                        "ammo":"9mm",
                        "skill":"pistol",
                        "range":12,
                        "ranged_damage":{"damage_type":"bullet","amount":2},
                        "dispersion":480,
                        "durability":6,
                        "min_cycle_recoil":350,
                        "modes":[["DEFAULT","semi",1]]
                    }
                },
                {
                    "type":"GENERIC",
                    "id":"combat_knife",
                    "name":"combat knife",
                    "flags":["SHEATH_KNIFE"],
                    "material":["steel"],
                    "qualities":[["CUT",1]],
                    "melee_damage":[{"damage_type":"cut","amount":18},{"damage_type":"stab","amount":8}],
                    "to_hit":1,
                    "attack_cost":85,
                    "techniques":["RAPID","WBLOCK_1"]
                },
                {
                    "type":"json_flag",
                    "id":"SHEATH_KNIFE",
                    "info":"Can be sheathed in a knife sheath."
                },
                {
                    "type":"material",
                    "id":"steel",
                    "name":"steel",
                    "density":7.8,
                    "bash_resist":9,
                    "cut_resist":10,
                    "fire_resist":8,
                    "repaired_with":"welder"
                },
                {
                    "type":"technique",
                    "id":"RAPID",
                    "name":"Rapid strike",
                    "description":"Attacks quickly."
                },
                {
                    "type":"TOOL",
                    "id":"flashlight",
                    "name":"flashlight",
                    "tool_data":{
                        "ammo":"battery",
                        "max_charges":100,
                        "initial_charges":20,
                        "charges_per_use":1,
                        "turns_per_charge":1,
                        "revert_to":"flashlight_off",
                        "subtype":"battery"
                    },
                    "use_action":{
                        "type":"transform",
                        "target":"flashlight_on",
                        "menu_text":"Turn on",
                        "msg":"The flashlight turns on.",
                        "need_charges":1,
                        "charges_to_use":1,
                        "moves":100
                    }
                },
                {
                    "type":"MAGAZINE",
                    "id":"glockmag",
                    "name":"Glock magazine",
                    "magazine_data":{
                        "ammo_type":"9mm",
                        "capacity":15,
                        "reload_time":100,
                        "linkage":"linkage_9mm"
                    }
                },
                {
                    "type":"BOOK",
                    "id":"manual_mechanics",
                    "name":"mechanics manual",
                    "book_data":{
                        "skill":"mechanics",
                        "required_level":1,
                        "max_level":3,
                        "intelligence":8,
                        "time":"30 m",
                        "fun":1,
                        "chapters":10
                    },
                    "proficiencies":["prof_lockpicking"]
                },
                {
                    "type":"skill",
                    "id":"mechanics",
                    "name":"mechanics",
                    "description":"Repair and build mechanical devices."
                },
                {
                    "type":"proficiency",
                    "id":"prof_lockpicking",
                    "name":"lockpicking",
                    "description":"Knowledge of opening locks without a key."
                },
                {
                    "type":"COMESTIBLE",
                    "id":"aspirin",
                    "name":"aspirin",
                    "comestible_type":"MED",
                    "calories":0,
                    "quench":0,
                    "healthy":-1,
                    "fun":0,
                    "spoils_in":"never",
                    "addiction_type":"none",
                    "vitamins":[["vitC", 1]]
                },
                {
                    "type":"vitamin",
                    "id":"vitC",
                    "name":"vitamin C",
                    "description":"Supports immune function."
                },
                {
                    "type":"GENERIC",
                    "id":"seed_corn",
                    "name":"corn seed",
                    "seed_data":{
                        "plant_name":"corn",
                        "fruit":"corn",
                        "byproducts":["straw_pile"],
                        "grow":"91 days",
                        "fruit_div":2,
                        "required_terrain":"t_dirt"
                    }
                }
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pack = dataset.get("hiking_pack").expect("pack");
        assert!(
            pack.fields
                .iter()
                .any(|(key, value)| key == "pocket_data" && value.contains("20 L"))
        );
        assert!(pack.fields.iter().any(|(key, value)| {
            key == "pocket_summary"
                && value.contains("CONTAINER")
                && value.contains("volume: 20 L")
                && value.contains("weight: 5 kg")
                && value.contains("length: 1 m")
                && value.contains("moves: 100")
                && value.contains("rigid: true")
        }));
        assert!(pack.fields.iter().any(|(key, value)| {
            key == "pocket_summary" && value.contains("MAGAZINE") && value.contains("ammo: 9mm")
        }));
        assert!(
            pack.fields
                .iter()
                .any(|(key, value)| key == "relative" && value.contains("80 g"))
        );
        assert!(
            pack.fields
                .iter()
                .any(|(key, value)| key == "delete" && value.contains("OLD_FLAG"))
        );
        assert!(
            pack.fields
                .iter()
                .any(|(key, value)| key == "extend" && value.contains("NEW_FLAG"))
        );
        assert_eq!(search_dataset(&dataset, "pocket_data", 10).len(), 1);
        assert_eq!(search_dataset(&dataset, "pocket_summary", 10).len(), 1);
        assert_eq!(search_dataset(&dataset, "pocket_summary 9mm", 10).len(), 1);
        assert!(
            search_dataset(&dataset, "9mm", 10)
                .iter()
                .any(|result| result.id == "hiking_pack")
        );

        let armor = dataset.get("leather_jacket").expect("armor");
        assert!(armor.fields.iter().any(|(key, value)| {
            key == "armor_summary"
                && value.contains("leather")
                && value.contains("enc: 12")
                && value.contains("coverage: 95")
                && value.contains("covers: torso, arm_l, arm_r")
                && value.contains("thickness: 2")
                && value.contains("env: 1")
                && value.contains("storage: 1 L")
        }));
        let gun = dataset.get("pistol_9mm").expect("gun");
        assert!(gun.fields.iter().any(|(key, value)| {
            key == "gun_summary"
                && value.contains("ammo: 9mm")
                && value.contains("skill: pistol")
                && value.contains("range: 12")
                && value.contains("dispersion: 480")
                && value.contains("cycle recoil: 350")
        }));
        assert_eq!(search_dataset(&dataset, "armor_summary torso", 10).len(), 1);
        assert_eq!(search_dataset(&dataset, "gun_summary pistol", 10).len(), 1);

        let knife = dataset.get("combat_knife").expect("knife");
        assert!(knife.fields.iter().any(|(key, value)| {
            key == "melee_summary"
                && value.contains("damage: cut 18, stab 8")
                && value.contains("to hit: 1")
                && value.contains("attack cost: 85")
                && value.contains("techniques: RAPID, WBLOCK_1")
        }));
        assert!(knife.fields.iter().any(|(key, value)| {
            key == "flag_summary"
                && value.contains("SHEATH_KNIFE")
                && value.contains("knife sheath")
        }));
        assert!(knife.fields.iter().any(|(key, value)| {
            key == "material_summary"
                && value.contains("steel")
                && value.contains("density: 7.8")
                && value.contains("bash resist: 9")
                && value.contains("cut resist: 10")
                && value.contains("repaired with: welder")
        }));
        assert!(knife.fields.iter().any(|(key, value)| {
            key == "technique_summary"
                && value.contains("RAPID")
                && value.contains("Rapid strike")
                && value.contains("Attacks quickly")
        }));
        assert!(
            knife
                .fields
                .iter()
                .any(|(key, value)| { key == "quality_summary" && value == "CUT: level 1" })
        );
        assert_eq!(search_dataset(&dataset, "melee_summary RAPID", 10).len(), 1);
        assert_eq!(search_dataset(&dataset, "flag_summary sheath", 10).len(), 1);
        assert_eq!(
            search_dataset(&dataset, "material_summary welder", 10).len(),
            1
        );
        assert_eq!(
            search_dataset(&dataset, "technique_summary rapid", 10).len(),
            1
        );
        assert_eq!(search_dataset(&dataset, "quality_summary CUT", 10).len(), 1);

        let tool = dataset.get("flashlight").expect("tool");
        assert!(tool.fields.iter().any(|(key, value)| {
            key == "use_action_summary"
                && value.contains("transform")
                && value.contains("target: flashlight_on")
                && value.contains("menu: Turn on")
                && value.contains("needs charges: 1")
                && value.contains("moves: 100")
        }));
        assert!(tool.fields.iter().any(|(key, value)| {
            key == "tool_summary"
                && value.contains("ammo: battery")
                && value.contains("max charges: 100")
                && value.contains("initial charges: 20")
                && value.contains("charges/use: 1")
                && value.contains("reverts to: flashlight_off")
        }));
        let magazine = dataset.get("glockmag").expect("magazine");
        assert!(magazine.fields.iter().any(|(key, value)| {
            key == "magazine_summary"
                && value.contains("ammo: 9mm")
                && value.contains("capacity: 15")
                && value.contains("reload time: 100")
                && value.contains("linkage: linkage_9mm")
        }));
        let book = dataset.get("manual_mechanics").expect("book");
        assert!(book.fields.iter().any(|(key, value)| {
            key == "book_summary"
                && value.contains("skill: mechanics")
                && value.contains("required: 1")
                && value.contains("max: 3")
                && value.contains("int: 8")
                && value.contains("chapters: 10")
        }));
        assert!(book.fields.iter().any(|(key, value)| {
            key == "skill_summary"
                && value.contains("mechanics")
                && value.contains("mechanical devices")
        }));
        assert!(book.fields.iter().any(|(key, value)| {
            key == "proficiency_summary"
                && value.contains("prof_lockpicking")
                && value.contains("opening locks")
        }));
        assert_eq!(
            search_dataset(&dataset, "tool_summary battery", 10).len(),
            1
        );
        assert_eq!(
            search_dataset(&dataset, "magazine_summary capacity", 10).len(),
            1
        );
        assert_eq!(
            search_dataset(&dataset, "book_summary mechanics", 10).len(),
            1
        );
        assert_eq!(
            search_dataset(&dataset, "skill_summary mechanical", 10).len(),
            1
        );
        assert_eq!(
            search_dataset(&dataset, "proficiency_summary locks", 10).len(),
            1
        );
        assert_eq!(
            search_dataset(&dataset, "use_action_summary flashlight_on", 10).len(),
            1
        );

        let aspirin = dataset.get("aspirin").expect("aspirin");
        assert!(aspirin.fields.iter().any(|(key, value)| {
            key == "comestible_summary"
                && value.contains("type: MED")
                && value.contains("healthy: -1")
                && value.contains("spoils in: never")
                && value.contains("addiction: none")
                && value.contains("vitamins: vitC, 1")
        }));
        assert!(aspirin.fields.iter().any(|(key, value)| {
            key == "vitamin_summary"
                && value.contains("vitC")
                && value.contains("vitamin C")
                && value.contains("immune function")
        }));
        let seed = dataset.get("seed_corn").expect("seed");
        assert!(seed.fields.iter().any(|(key, value)| {
            key == "seed_summary"
                && value.contains("plant: corn")
                && value.contains("fruit: corn")
                && value.contains("byproducts: straw_pile")
                && value.contains("grow: 91 days")
                && value.contains("terrain: t_dirt")
        }));
        assert_eq!(
            search_dataset(&dataset, "comestible_summary MED", 10).len(),
            1
        );
        assert_eq!(
            search_dataset(&dataset, "vitamin_summary immune", 10).len(),
            1
        );
        assert_eq!(search_dataset(&dataset, "seed_summary corn", 10).len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_resolves_copy_from_fields() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-inherit-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {
                    "type":"GENERIC",
                    "abstract":"base_pole",
                    "id":"base_pole",
                    "name":"base pole",
                    "volume":"750 ml",
                    "weight":"700 g",
                    "material":["wood"],
                    "flags":["SPEAR"]
                },
                {
                    "type":"GENERIC",
                    "id":"long_pole",
                    "copy-from":"base_pole",
                    "name":"long pole",
                    "weight":"900 g",
                    "extend":{"flags":["DURABLE"]},
                    "delete":{"flags":["SPEAR"]}
                },
                {
                    "type":"GENERIC",
                    "id":"short_pole",
                    "copy-from":"base_pole",
                    "name":"short pole",
                    "relative":{"volume":"-250 ml","weight":"50%"}
                }
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pole = dataset.get("long_pole").expect("long pole");
        assert!(
            pole.fields
                .iter()
                .any(|(key, value)| key == "volume" && value == "750 ml")
        );
        assert!(
            pole.fields
                .iter()
                .any(|(key, value)| key == "weight" && value == "900 g")
        );
        assert!(
            pole.fields
                .iter()
                .any(|(key, value)| key == "material" && value.contains("wood"))
        );
        assert!(
            pole.fields
                .iter()
                .any(|(key, value)| key == "copy-from" && value == "base_pole")
        );
        let flags = pole
            .fields
            .iter()
            .find_map(|(key, value)| (key == "flags").then_some(value))
            .expect("flags");
        assert!(flags.contains("DURABLE"));
        assert!(!flags.contains("SPEAR"));

        let short = dataset.get("short_pole").expect("short pole");
        assert!(
            short
                .fields
                .iter()
                .any(|(key, value)| key == "volume" && value == "500 ml")
        );
        assert!(
            short
                .fields
                .iter()
                .any(|(key, value)| key == "weight" && value == "1050 g")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_dataset_ranks_exact_ids_before_field_matches() {
        let dataset = GuideDataset {
            entries: vec![
                GuideSearchResult {
                    id: "stick_long".to_string(),
                    kind: "GENERIC".to_string(),
                    name: "long stick".to_string(),
                    description: String::new(),
                    fields: vec![("used_by_recipe".to_string(), "long_pole".to_string())],
                    raw_json: String::new(),
                },
                GuideSearchResult {
                    id: "long_pole".to_string(),
                    kind: "GENERIC".to_string(),
                    name: "long pole".to_string(),
                    description: String::new(),
                    fields: Vec::new(),
                    raw_json: String::new(),
                },
            ],
            language: "en".to_string(),
            warning: None,
        };

        let results = search_dataset(&dataset, "long_pole", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("long_pole")
        );
    }

    #[test]
    fn resolve_build_maps_release_tags_to_guide_build_numbers() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-resolve-test-{}", std::process::id()));
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(&cache).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[
                {"build_number":"cdda-0.I-2026-03-05-0143","prerelease":true,"langs":[]},
                {"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}
            ]"#,
        )
        .expect("builds cache");

        assert_eq!(
            resolve_build(&root, "0.H", "stable").expect("stable build"),
            "0.H-RELEASE"
        );
        assert_eq!(
            resolve_build(&root, "0.I", "experimental").expect("0.I build"),
            "cdda-0.I-2026-03-05-0143"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_adds_recipe_relationships_and_raw_json() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-derived-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"GENERIC","id":"stick_long","name":"long stick"},
                {"type":"GENERIC","id":"wood_splinter","name":"wood splinter"},
                {"type":"TOOL","id":"hammer","name":"hammer","qualities":[["HAMMER",1]]},
                {
                    "type":"recipe",
                    "result":"long_pole",
                    "components":[[["stick_long",1]]],
                    "tools":[[["HAMMER",1]]],
                    "byproducts":[["wood_splinter",1]],
                    "time":"10 m"
                }
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pole = search_dataset(&dataset, "long_pole", 10)
            .into_iter()
            .find(|item| item.id == "long_pole")
            .expect("long pole");
        assert!(pole.raw_json.contains("long_pole"));
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "crafted_by"
                && value.contains("recipe/long_pole")
                && value.contains("stick_long")
                && value.contains("10 m")
        }));
        assert_eq!(
            dataset.get("recipe/long_pole").expect("recipe entry").kind,
            "recipe"
        );
        assert!(
            relation_target_ids(&pole)
                .iter()
                .any(|target| target == "recipe/long_pole")
        );

        let stick = search_dataset(&dataset, "used_by_recipe", 10)
            .into_iter()
            .find(|item| item.id == "stick_long")
            .expect("stick");
        assert!(
            stick
                .fields
                .iter()
                .any(|(key, value)| key == "used_by_recipe" && value.contains("long_pole"))
        );
        let hammer = dataset.get("hammer").expect("hammer");
        assert!(hammer.fields.iter().any(|(key, value)| {
            key == "tool_for_recipe" && value == "recipe/long_pole -> long_pole; quality: HAMMER"
        }));
        let splinter = dataset.get("wood_splinter").expect("splinter");
        assert!(splinter.fields.iter().any(|(key, value)| {
            key == "byproduct_of_recipe" && value == "recipe/long_pole -> long_pole"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_expands_recipe_requirements() {
        let root = std::env::temp_dir().join(format!(
            "cddock-guide-requirement-test-{}",
            std::process::id()
        ));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"GENERIC","id":"stick_long","name":"long stick"},
                {"type":"GENERIC","id":"rag","name":"rag"},
                {"type":"TOOL","id":"hammer","name":"hammer","qualities":[["HAMMER",1]]},
                {"type":"requirement","id":"req_binding","components":[[["rag",1]]]},
                {"type":"requirement","id":"req_pole_parts","components":[[["stick_long",1]]],"tools":[[["hammer",1]]]},
                {"type":"requirement","id":"req_full_pole","using":[["req_pole_parts",1],["req_binding",1]]},
                {"type":"recipe","result":"long_pole","using":[["req_full_pole",1]],"time":"10 m"}
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pole = dataset.get("long_pole").expect("long pole");
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "crafted_by"
                && value.contains("stick_long")
                && value.contains("rag")
                && value.contains("hammer")
                && value.contains("recipe/long_pole")
        }));

        let stick = dataset.get("stick_long").expect("stick");
        assert!(stick.fields.iter().any(|(key, value)| {
            key == "used_by_recipe" && value == "recipe/long_pole -> long_pole"
        }));
        let rag = dataset.get("rag").expect("rag");
        assert!(rag.fields.iter().any(|(key, value)| {
            key == "used_by_recipe" && value == "recipe/long_pole -> long_pole"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_adds_item_group_relationships() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-group-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"item_group","id":"tools_common","subtype":"collection","items":[["long_pole", 25]]}
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pole = search_dataset(&dataset, "tools_common", 10)
            .into_iter()
            .find(|item| item.id == "long_pole")
            .expect("long pole");
        assert!(
            pole.fields
                .iter()
                .any(|(key, value)| { key == "found_in_group" && value.contains("tools_common") })
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_expands_nested_item_group_relationships() {
        let root = std::env::temp_dir().join(format!(
            "cddock-guide-nested-group-test-{}",
            std::process::id()
        ));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"item_group","id":"tools_poles","subtype":"collection","items":[["long_pole", 25]]},
                {"type":"item_group","id":"garage_tools","subtype":"distribution","items":[["tools_poles", 100]]}
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pole = dataset.get("long_pole").expect("long pole");
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "found_in_group" && value == "tools_poles (collection)"
        }));
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "found_in_group" && value == "garage_tools (distribution) via tools_poles"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_adds_ammo_and_magazine_relationships() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-ammo-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"AMMO","id":"9mm","name":"9mm"},
                {
                    "type":"MAGAZINE",
                    "id":"glockmag",
                    "name":"Glock magazine",
                    "pocket_data":[{"pocket_type":"MAGAZINE","ammo_restriction":{"9mm":15}}]
                },
                {
                    "type":"GUN",
                    "id":"glock",
                    "name":"Glock",
                    "ammo":"9mm",
                    "magazines":[["glockmag"]]
                }
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let ammo = dataset.get("9mm").expect("9mm");
        assert!(
            ammo.fields
                .iter()
                .any(|(key, value)| key == "ammo_used_by" && value == "glock")
        );
        assert!(
            ammo.fields
                .iter()
                .any(|(key, value)| key == "ammo_contained_by" && value == "glockmag")
        );

        let magazine = dataset.get("glockmag").expect("magazine");
        assert!(
            magazine
                .fields
                .iter()
                .any(|(key, value)| key == "magazine_for" && value == "glock")
        );
        assert!(
            magazine
                .fields
                .iter()
                .any(|(key, value)| key == "contains_ammo" && value == "9mm")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_adds_construction_and_vehicle_part_relationships() {
        let root = std::env::temp_dir().join(format!(
            "cddock-guide-construction-test-{}",
            std::process::id()
        ));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"GENERIC","id":"rag","name":"rag"},
                {"type":"TOOL","id":"hammer","name":"hammer","qualities":[["HAMMER",1]]},
                {"type":"GENERIC","id":"bike_wheel","name":"bike wheel"},
                {"type":"requirement","id":"req_rack_parts","components":[[["long_pole",2],["rag",1]]],"tools":[[["HAMMER",1]]]},
                {
                    "type":"construction",
                    "id":"constr_long_pole_rack",
                    "using":[["req_rack_parts",1]],
                    "pre_terrain":"t_floor",
                    "post_terrain":"t_rack",
                    "time":"20 m",
                    "difficulty":2
                },
                {
                    "type":"vehicle_part",
                    "id":"wheel_bicycle",
                    "name":"bicycle wheel",
                    "item":"bike_wheel",
                    "location":"on_roof",
                    "durability":80,
                    "flags":["WHEEL"]
                }
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pole = dataset.get("long_pole").expect("long pole");
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "used_in_construction"
                && value.contains("construction:constr_long_pole_rack")
                && value.contains("from: t_floor")
                && value.contains("to: t_rack")
                && value.contains("time: 20 m")
        }));
        let hammer = dataset.get("hammer").expect("hammer");
        assert!(hammer.fields.iter().any(|(key, value)| {
            key == "tool_for_construction"
                && value.contains("construction:constr_long_pole_rack")
                && value.contains("quality: HAMMER")
        }));
        let wheel = dataset.get("bike_wheel").expect("wheel");
        assert!(wheel.fields.iter().any(|(key, value)| {
            key == "installed_as_vehicle_part"
                && value.contains("vehicle_part:wheel_bicycle")
                && value.contains("location: on_roof")
                && value.contains("durability: 80")
                && value.contains("flags: WHEEL")
        }));
        assert_eq!(
            search_dataset(&dataset, "used_in_construction rack", 10).len(),
            2
        );
        assert_eq!(
            search_dataset(&dataset, "installed_as_vehicle_part wheel", 10).len(),
            1
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_adds_monster_source_relationships() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-monster-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"item_group","id":"zombie_drops","items":[["long_pole", 10]]},
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"COMESTIBLE","id":"meat","name":"meat"},
                {"type":"harvest","id":"zombie_harvest","entries":[{"drop":"meat","type":"flesh","mass_ratio":0.4}]},
                {"type":"MONSTER","id":"mon_zombie","name":"zombie","death_drops":"zombie_drops","harvest":"zombie_harvest"},
                {"type":"monstergroup","name":"GROUP_ZOMBIE","monsters":[{"monster":"mon_zombie","freq":100}]}
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let group = search_dataset(&dataset, "monster_source", 10)
            .into_iter()
            .find(|item| item.id == "zombie_drops")
            .expect("drop group");
        assert!(
            group
                .fields
                .iter()
                .any(|(key, value)| key == "monster_source" && value.contains("mon_zombie"))
        );
        let meat = dataset.get("meat").expect("meat");
        assert!(meat.fields.iter().any(|(key, value)| {
            key == "harvested_from"
                && value.contains("harvest:zombie_harvest")
                && value.contains("monsters: mon_zombie")
        }));
        assert_eq!(
            search_dataset(&dataset, "harvested_from zombie", 10).len(),
            1
        );

        let monster = search_dataset(&dataset, "GROUP_ZOMBIE", 10)
            .into_iter()
            .find(|item| item.id == "mon_zombie")
            .expect("monster");
        assert!(
            monster
                .fields
                .iter()
                .any(|(key, value)| key == "monster_group" && value == "GROUP_ZOMBIE")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_derives_relationships_from_resolved_copy_from() {
        let root = std::env::temp_dir().join(format!(
            "cddock-guide-derived-inherit-test-{}",
            std::process::id()
        ));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"item_group","id":"zombie_drops","items":[["long_pole", 10]]},
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"MONSTER","abstract":"base_zombie","id":"base_zombie","death_drops":"zombie_drops"},
                {"type":"MONSTER","id":"mon_zombie","copy-from":"base_zombie","name":"zombie"}
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let group = dataset.get("zombie_drops").expect("drop group");
        assert!(group.fields.iter().any(|(key, value)| {
            key == "monster_source" && value == "mon_zombie via death_drops"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_dataset_adds_generic_reverse_references() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-ref-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[
                {"type":"GENERIC","id":"long_pole","name":"long pole"},
                {"type":"construction","id":"constr_long_pole_rack","using":[["long_pole", 1]]},
                {"type":"mapgen","id":"test_house","place_items":[{"item":"long_pole","x":1,"y":2}]},
                {"type":"map_extra","id":"mx_pole_cache","items":[["long_pole", 100]]},
                {"type":"overmap_special","id":"oms_pole_yard","locations":["field"],"city_distance":[0,4],"items":["long_pole"]},
                {"type":"effect_on_condition","id":"EOC_GRANT_POLE","effect":[{"u_add_item":"long_pole"}]}
            ]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        let pole = search_dataset(&dataset, "constr_long_pole_rack", 10)
            .into_iter()
            .find(|item| item.id == "long_pole")
            .expect("long pole");
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "referenced_by" && value.contains("construction:constr_long_pole_rack")
        }));
        assert!(
            pole.fields
                .iter()
                .any(|(key, value)| { key == "placed_by_mapgen" && value == "mapgen:test_house" })
        );
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "placed_by_map_extra" && value == "map_extra:mx_pole_cache"
        }));
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "placed_by_overmap_special" && value == "overmap_special:oms_pole_yard"
        }));
        assert!(pole.fields.iter().any(|(key, value)| {
            key == "referenced_by_eoc" && value == "effect_on_condition:EOC_GRANT_POLE"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn dataset_get_returns_entries_by_id() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-get-test-{}", std::process::id()));
        let build = "0.H-RELEASE";
        let cache = guide_cache_dir(&root);
        fs::create_dir_all(cache.join(build)).expect("cache dir");
        fs::write(
            cache.join("builds.json"),
            r#"[{"build_number":"0.H-RELEASE","prerelease":false,"langs":["zh_CN"]}]"#,
        )
        .expect("builds cache");
        fs::write(
            cache.join(build).join("all.json"),
            r#"[{"type":"GENERIC","id":"long_pole","name":"long pole"}]"#,
        )
        .expect("all cache");

        let dataset = load_dataset(&root, build, "en").expect("dataset");
        assert_eq!(
            dataset.get("long_pole").expect("long pole").name,
            "long pole"
        );
        assert!(dataset.contains_id("long_pole"));
        assert!(dataset.get("missing").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn relation_target_ids_extracts_navigable_ids() {
        let result = GuideSearchResult {
            id: "long_pole".to_string(),
            kind: "GENERIC".to_string(),
            name: "long pole".to_string(),
            description: String::new(),
            fields: vec![
                (
                    "referenced_by".to_string(),
                    "construction:constr_long_pole_rack".to_string(),
                ),
                (
                    "installed_as_vehicle_part".to_string(),
                    "vehicle_part:wheel_bicycle".to_string(),
                ),
                (
                    "placed_by_mapgen".to_string(),
                    "mapgen:test_house".to_string(),
                ),
                (
                    "used_by_recipe".to_string(),
                    "stick_long -> long_pole".to_string(),
                ),
                (
                    "monster_source".to_string(),
                    "mon_zombie via death_drops".to_string(),
                ),
                (
                    "found_in_group".to_string(),
                    "tools_common (collection)".to_string(),
                ),
            ],
            raw_json: String::new(),
        };

        let targets = relation_target_ids(&result);
        assert!(targets.contains(&"constr_long_pole_rack".to_string()));
        assert!(targets.contains(&"wheel_bicycle".to_string()));
        assert!(targets.contains(&"test_house".to_string()));
        assert!(targets.contains(&"stick_long".to_string()));
        assert!(targets.contains(&"mon_zombie".to_string()));
        assert!(targets.contains(&"tools_common".to_string()));
        assert!(!targets.contains(&"long_pole".to_string()));
        assert!(!targets.contains(&"construction".to_string()));
        assert!(!targets.contains(&"mapgen".to_string()));
        assert!(!targets.contains(&"vehicle_part".to_string()));
        assert!(!targets.contains(&"collection".to_string()));
        assert!(!targets.contains(&"death_drops".to_string()));
    }

    #[test]
    fn field_target_ids_includes_generic_data_references() {
        let result = GuideSearchResult {
            id: "long_pole".to_string(),
            kind: "GENERIC".to_string(),
            name: "long pole".to_string(),
            description: String::new(),
            fields: vec![
                ("looks_like".to_string(), "stick_long".to_string()),
                (
                    "use_action".to_string(),
                    "target: fire; item: charcoal".to_string(),
                ),
                (
                    "tile_match".to_string(),
                    "tileset: TestTiles; fg: 42".to_string(),
                ),
            ],
            raw_json: String::new(),
        };

        let targets = field_target_ids(&result);
        assert!(targets.contains(&"stick_long".to_string()));
        assert!(targets.contains(&"charcoal".to_string()));
        assert!(!targets.contains(&"long_pole".to_string()));
        assert!(!targets.contains(&"TestTiles".to_string()));
    }

    #[test]
    fn add_local_tile_info_reads_tile_config_matches() {
        let root =
            std::env::temp_dir().join(format!("cddock-guide-tile-test-{}", std::process::id()));
        let build = "test-build";
        let tileset = build_dir(&root, build).join("gfx").join("TestTiles");
        fs::create_dir_all(&tileset).expect("tileset dir");
        fs::write(
            tileset.join("tile_config.json"),
            r#"{
                "tile_info": [{"width": 32, "height": 32}],
                "tiles-new": [
                    {
                        "file": "items.png",
                        "tiles": [
                            {"id": ["long_pole", "stick_long"], "fg": 42, "bg": 0}
                        ]
                    }
                ]
            }"#,
        )
        .expect("tile config");
        let image = image::RgbaImage::from_pixel(64, 768, image::Rgba([255, 0, 0, 255]));
        image.save(tileset.join("items.png")).expect("png image");

        let mut result = GuideSearchResult {
            id: "long_pole".to_string(),
            kind: "GENERIC".to_string(),
            name: "long pole".to_string(),
            description: String::new(),
            fields: Vec::new(),
            raw_json: String::new(),
        };
        add_local_tile_info(&root, build, &mut result);

        assert!(result.fields.iter().any(|(key, value)| {
            key == "tile_match"
                && value.contains("TestTiles")
                && value.contains("items.png")
                && value.contains("fg: 42")
                && value.contains("fg_crop: 0,672 32x32")
                && value.contains("fg_preview:")
        }));
        assert!(
            guide_cache_dir(&root)
                .join("tiles")
                .join("long_pole")
                .is_dir()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_local_tile_info_uses_looks_like_tile_fallback() {
        let root = std::env::temp_dir().join(format!(
            "cddock-guide-tile-fallback-test-{}",
            std::process::id()
        ));
        let build = "test-build";
        let tileset = build_dir(&root, build).join("gfx").join("FallbackTiles");
        fs::create_dir_all(&tileset).expect("tileset dir");
        fs::write(
            tileset.join("tile_config.json"),
            r#"{
                "tile_info": [{"width": 16, "height": 16}],
                "tiles-new": [
                    {
                        "file": "items.png",
                        "tiles": [
                            {"id": "stick_long", "fg": 3}
                        ]
                    }
                ]
            }"#,
        )
        .expect("tile config");
        let image = image::RgbaImage::from_pixel(64, 16, image::Rgba([0, 0, 255, 255]));
        image.save(tileset.join("items.png")).expect("png image");

        let mut result = GuideSearchResult {
            id: "long_pole".to_string(),
            kind: "GENERIC".to_string(),
            name: "long pole".to_string(),
            description: String::new(),
            fields: vec![("looks_like".to_string(), "stick_long".to_string())],
            raw_json: String::new(),
        };
        add_local_tile_info(&root, build, &mut result);

        let tile = result
            .fields
            .iter()
            .find_map(|(key, value)| (key == "tile_match").then_some(value))
            .expect("tile match");
        assert!(tile.contains("matched_id: stick_long"));
        assert!(tile.contains("FallbackTiles"));
        assert!(tile.contains("fg_crop: 48,0 16x16"));
        assert!(tile.contains("fg_preview:"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_local_tile_info_reads_shared_userdata_gfx() {
        let root = std::env::temp_dir().join(format!(
            "cddock-guide-shared-gfx-test-{}",
            std::process::id()
        ));
        let build = "test-build";
        let tileset = shared_userdata_dir(&root).join("gfx").join("SharedTiles");
        fs::create_dir_all(&tileset).expect("tileset dir");
        fs::write(
            tileset.join("tile_config.json"),
            r#"{
                "tile_info": [{"width": 16, "height": 16}],
                "tiles-new": [
                    {
                        "file": "items.png",
                        "tiles": [
                            {"id": "long_pole", "fg": 2}
                        ]
                    }
                ]
            }"#,
        )
        .expect("tile config");
        let image = image::RgbaImage::from_pixel(64, 16, image::Rgba([255, 255, 0, 255]));
        image.save(tileset.join("items.png")).expect("png image");

        let mut result = GuideSearchResult {
            id: "long_pole".to_string(),
            kind: "GENERIC".to_string(),
            name: "long pole".to_string(),
            description: String::new(),
            fields: Vec::new(),
            raw_json: String::new(),
        };
        add_local_tile_info(&root, build, &mut result);

        let tile = result
            .fields
            .iter()
            .find_map(|(key, value)| (key == "tile_match").then_some(value))
            .expect("tile match");
        assert!(tile.contains("SharedTiles"));
        assert!(tile.contains("fg_crop: 32,0 16x16"));
        assert!(tile.contains("fg_preview:"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_local_tile_info_exports_array_fg_and_bg_previews() {
        let root = std::env::temp_dir().join(format!(
            "cddock-guide-tile-array-test-{}",
            std::process::id()
        ));
        let build = "test-build";
        let tileset = build_dir(&root, build).join("gfx").join("LayeredTiles");
        fs::create_dir_all(&tileset).expect("tileset dir");
        fs::write(
            tileset.join("tile_config.json"),
            r#"{
                "tile_info": [{"width": 16, "height": 16}],
                "tiles-new": [
                    {
                        "file": "items.png",
                        "tiles": [
                            {
                                "id": "long_pole",
                                "fg": [5, 6],
                                "bg": {"sprite": 2},
                                "additional_tiles": [
                                    {"id": "open", "fg": 7, "bg": 1}
                                ]
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("tile config");
        let image = image::RgbaImage::from_pixel(64, 32, image::Rgba([0, 255, 0, 255]));
        image.save(tileset.join("items.png")).expect("png image");

        let mut result = GuideSearchResult {
            id: "long_pole".to_string(),
            kind: "GENERIC".to_string(),
            name: "long pole".to_string(),
            description: String::new(),
            fields: Vec::new(),
            raw_json: String::new(),
        };
        add_local_tile_info(&root, build, &mut result);

        let tile = result
            .fields
            .iter()
            .find_map(|(key, value)| (key == "tile_match").then_some(value))
            .expect("tile match");
        assert!(tile.contains("fg_crop: 16,16 16x16"));
        assert!(tile.contains("bg_crop: 32,0 16x16"));
        assert!(tile.contains("additional_open_fg_crop: 48,16 16x16"));
        assert!(tile.contains("additional_open_bg_crop: 16,0 16x16"));
        assert_eq!(tile.matches("preview:").count(), 4);

        let _ = fs::remove_dir_all(root);
    }
}
