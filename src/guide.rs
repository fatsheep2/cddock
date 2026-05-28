use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{http, paths::guide_cache_dir};

const BUILDS_URL: &str = "https://raw.githubusercontent.com/nornagon/cdda-data/main/builds.json";
const DATA_BASE_URL: &str = "https://raw.githubusercontent.com/nornagon/cdda-data/main/data";

#[derive(Debug, Clone, Deserialize)]
pub struct GuideBuild {
    pub build_number: String,
    pub prerelease: bool,
    #[serde(default)]
    #[serde(rename = "langs")]
    pub _langs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GuideSearchResult {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub fields: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct GuideDataset {
    entries: Vec<GuideSearchResult>,
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
        return Ok(active_build.trim().to_string());
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

pub fn fetch_builds(game_root: &Path) -> Result<Vec<GuideBuild>, String> {
    let cache_path = guide_cache_dir(game_root).join("builds.json");
    let text = fetch_cached(BUILDS_URL, &cache_path)?;
    serde_json::from_str(&text).map_err(|error| format!("Failed to parse guide builds: {error}"))
}

pub fn load_dataset(game_root: &Path, build: &str, language: &str) -> Result<GuideDataset, String> {
    let data = load_json(game_root, build, "all.json")?;
    let translations = load_translations(game_root, build, language).unwrap_or_default();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    collect_entries(&data, &translations, &mut seen, &mut entries);
    Ok(GuideDataset { entries })
}

pub fn search_dataset(dataset: &GuideDataset, query: &str, limit: usize) -> Vec<GuideSearchResult> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let query = query.to_lowercase();
    dataset
        .entries
        .iter()
        .filter(|result| {
            format!(
                "{}\n{}\n{}\n{}",
                result.id, result.kind, result.name, result.description
            )
            .to_lowercase()
            .contains(&query)
        })
        .take(limit)
        .cloned()
        .collect()
}

fn load_json(game_root: &Path, build: &str, relative: &str) -> Result<Value, String> {
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

fn load_translations(
    game_root: &Path,
    build: &str,
    language: &str,
) -> Result<HashMap<String, String>, String> {
    if language == "en" {
        return Ok(HashMap::new());
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
    value: &Value,
    translations: &HashMap<String, String>,
    seen: &mut HashSet<String>,
    entries: &mut Vec<GuideSearchResult>,
) {
    match value {
        Value::Object(map) => {
            if let Some(result) = object_to_result(map, translations) {
                let key = format!("{}:{}", result.kind, result.id);
                if seen.insert(key) {
                    entries.push(result);
                }
            }
            for child in map.values() {
                collect_entries(child, translations, seen, entries);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_entries(child, translations, seen, entries);
            }
        }
        _ => {}
    }
}

fn object_to_result(
    map: &Map<String, Value>,
    translations: &HashMap<String, String>,
) -> Option<GuideSearchResult> {
    let id = field_text(map, "id", translations)?;
    let kind = field_text(map, "type", translations).unwrap_or_else(|| "entry".to_string());
    let name = field_text(map, "name", translations).unwrap_or_else(|| id.clone());
    let description = field_text(map, "description", translations).unwrap_or_default();
    let mut fields = Vec::new();

    for key in [
        "symbol",
        "color",
        "volume",
        "weight",
        "material",
        "flags",
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
        "harvest",
        "death_function",
    ] {
        if let Some(value) = map
            .get(key)
            .and_then(|value| compact_value(value, translations))
        {
            fields.push((key.to_string(), value));
        }
    }

    Some(GuideSearchResult {
        id,
        kind,
        name,
        description,
        fields,
    })
}

fn field_text(
    map: &Map<String, Value>,
    key: &str,
    translations: &HashMap<String, String>,
) -> Option<String> {
    map.get(key)
        .and_then(|value| compact_value(value, translations))
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
