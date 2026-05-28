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
    paths::{build_dir, guide_cache_dir},
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
    collect_entries(&data, &translations, &mut seen, &mut entries);
    add_derived_fields(&data, &mut entries, &translations);
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

pub fn add_local_tile_info(game_root: &Path, active_build: &str, result: &mut GuideSearchResult) {
    if active_build.trim().is_empty() {
        return;
    }
    let build_path = build_dir(game_root, active_build);
    let preview_dir = guide_cache_dir(game_root)
        .join("tiles")
        .join(safe_file_name(&result.id));
    let matches = find_tile_matches(&build_path, &result.id, &preview_dir);
    if matches.is_empty() {
        result.fields.push((
            "tile_match".to_string(),
            "no local tileset entry found under active build gfx/".to_string(),
        ));
        return;
    }

    for item in matches.into_iter().take(6) {
        result.fields.push(("tile_match".to_string(), item));
    }
}

fn find_tile_matches(build_path: &Path, id: &str, preview_dir: &Path) -> Vec<String> {
    let gfx = build_path.join("gfx");
    if !gfx.is_dir() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let Ok(tilesets) = fs::read_dir(gfx) else {
        return matches;
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
            &mut matches,
        );
    }
    matches
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
    if let (Some(sheet), Some((tile_width, tile_height)), Some(tile_id)) = (
        sheet,
        tile_size,
        map.get("fg").and_then(|value| value.as_u64()),
    ) {
        let sheet_path = tileset_dir.join(sheet);
        if let Some((image_width, _)) = png_dimensions(&sheet_path) {
            let columns = (image_width / tile_width).max(1);
            let tile_id = tile_id as u32;
            let x = (tile_id % columns) * tile_width;
            let y = (tile_id / columns) * tile_height;
            parts.push(format!("crop: {x},{y} {tile_width}x{tile_height}"));
            if let Some(preview) = export_tile_preview(
                &sheet_path,
                preview_dir,
                tileset,
                sheet,
                x,
                y,
                tile_width,
                tile_height,
            ) {
                parts.push(format!("preview: {}", preview.display()));
            }
        }
    }
    parts.join("; ")
}

fn export_tile_preview(
    sheet_path: &Path,
    preview_dir: &Path,
    tileset: &str,
    sheet: &str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Option<PathBuf> {
    let image = image::open(sheet_path).ok()?;
    let crop = image.crop_imm(x, y, width, height);
    fs::create_dir_all(preview_dir).ok()?;
    let filename = format!(
        "{}-{}-{}-{}.png",
        safe_file_name(tileset),
        safe_file_name(sheet),
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
    Some((width, height))
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
        raw_json: serde_json::to_string_pretty(&Value::Object(map.clone())).unwrap_or_default(),
    })
}

fn add_derived_fields(
    data: &Value,
    entries: &mut [GuideSearchResult],
    translations: &HashMap<String, String>,
) {
    let mut index = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        index.insert(entry.id.clone(), idx);
    }

    let mut objects = Vec::new();
    collect_objects(data, &mut objects);
    for map in objects {
        let kind = map
            .get("type")
            .and_then(|value| compact_value(value, translations))
            .unwrap_or_default();
        match kind.as_str() {
            "recipe" => add_recipe_fields(map, entries, &index, translations, false),
            "uncraft" => add_recipe_fields(map, entries, &index, translations, true),
            "item_group" => add_item_group_fields(map, entries, &index, translations),
            "MONSTER" => add_monster_fields(map, entries, &index, translations),
            "monstergroup" => add_monster_group_fields(map, entries, &index, translations),
            _ => {}
        }
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

fn add_item_group_fields(
    map: &Map<String, Value>,
    entries: &mut [GuideSearchResult],
    index: &HashMap<String, usize>,
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
    let items = map
        .get("items")
        .or_else(|| map.get("entries"))
        .map(extract_string_tokens)
        .unwrap_or_default();

    for item in items.iter().filter(|item| index.contains_key(*item)) {
        if let Some(target) = index.get(item).and_then(|idx| entries.get_mut(*idx)) {
            let label = if subtype.is_empty() {
                group_id.clone()
            } else {
                format!("{group_id} ({subtype})")
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
    translations: &HashMap<String, String>,
    uncraft: bool,
) {
    let Some(result) = map
        .get("result")
        .and_then(|value| compact_value(value, translations))
    else {
        return;
    };
    let recipe_name = map
        .get("id")
        .and_then(|value| compact_value(value, translations))
        .unwrap_or_else(|| result.clone());
    let time = map
        .get("time")
        .and_then(|value| compact_value(value, translations))
        .unwrap_or_default();
    let components = map
        .get("components")
        .map(extract_string_tokens)
        .unwrap_or_default();
    let tools = map
        .get("tools")
        .map(extract_string_tokens)
        .unwrap_or_default();
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
                {"type":"recipe","result":"long_pole","components":[[["stick_long",1]]],"time":"10 m"}
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
            key == "crafted_by" && value.contains("stick_long") && value.contains("10 m")
        }));

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
                {"type":"MONSTER","id":"mon_zombie","name":"zombie","death_drops":"zombie_drops"},
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
                && value.contains("crop: 0,672 32x32")
                && value.contains("preview:")
        }));
        assert!(
            guide_cache_dir(&root)
                .join("tiles")
                .join("long_pole")
                .is_dir()
        );

        let _ = fs::remove_dir_all(root);
    }
}
