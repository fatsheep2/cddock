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
    pub langs: Vec<String>,
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
    collect_entries(&data, &translations, &mut seen, &mut entries);
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
            let fields = result
                .fields
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}\n{}\n{}\n{}",
                result.id, result.kind, result.name, result.description
            )
            .to_lowercase()
            .contains(&query)
                || fields.to_lowercase().contains(&query)
        })
        .take(limit)
        .cloned()
        .collect()
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
        "abstract",
        "copy-from",
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
        "to_hit",
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
}
