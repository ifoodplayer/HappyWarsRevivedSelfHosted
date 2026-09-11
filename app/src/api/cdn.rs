use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::Response,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::api::payload;

const EMBEDDED_LIST_JSON: &str = include_str!("../config/database.json");

// ─── Config ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct RewardConfig {
    pub enabled: bool,
    pub txid_01: String,
    pub reward_set: String,
    pub ticket_num: u32,
    pub end_date: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub bind_ip: String,
    pub current_event: CurrentEvent,
    pub reward: RewardConfig,
}

#[derive(Debug, Clone)]
pub struct CurrentEvent {
    pub xbox360: u32,
    pub xbox_uwp: u32,
}

const DEFAULT_INI: &str = include_str!("../config/app.ini");

impl Config {
    pub fn load(path: &str) -> Self {
        let mut ini = configparser::ini::Ini::new();

        if ini.load(path).is_err() {
            if let Err(e) = std::fs::write(path, DEFAULT_INI) {
                eprintln!(" \x1b[33mWARN\x1b[0m [Config] Could not write {path}: {e}");
            } else {
                println!(" \x1b[32mINFO\x1b[0m [Config] Creating {path} ");
            }
            let _ = ini.read(DEFAULT_INI.to_string());
        }

        let port = crate::SERVER_PORT;
        let xbox360 = ini.getuint("Event", "Xbox360")
            .unwrap_or_default()
            .unwrap_or(2322) as u32;
        let xbox_uwp = ini.getuint("Event", "XboxUWP")
            .unwrap_or_default()
            .unwrap_or(2322) as u32;

        let reward = RewardConfig {
            enabled: ini.getbool("Reward", "Enabled")
                .unwrap_or_default()
                .unwrap_or(true),
            txid_01: ini.get("Reward", "TXID_01")
                .unwrap_or_else(|| "TXDLI00451".to_string()),
            reward_set: ini.get("Reward", "RewardSet")
                .unwrap_or_else(|| "LotteryBuffSet_0B".to_string()),
            ticket_num: ini.getuint("Reward", "TicketNum")
                .unwrap_or_default()
                .unwrap_or(1000) as u32,
            end_date: ini.get("Reward", "EndDate")
                .unwrap_or_else(|| "20270101".to_string()),
        };

        Self {
            port,
            bind_ip: "127.0.0.1".to_string(),
            current_event: CurrentEvent { xbox360, xbox_uwp },
            reward,
        }
    }

    pub fn with_bind_ip(mut self, bind_ip: String) -> Self {
        self.bind_ip = bind_ip;
        self
    }

    pub fn content_folder(&self) -> PathBuf {
        PathBuf::from("content")
    }

    pub fn event_id(&self, project_id: u8) -> u32 {
        match project_id {
            1 => self.current_event.xbox360,
            _ => self.current_event.xbox_uwp,
        }
    }

    pub fn cdn_base_url(&self) -> String {
        self.cdn_base_url_for_project(0)
    }

    pub fn cdn_base_url_for_project(&self, project_id: u8) -> String {
        let host = match project_id {
            1 | 2 => self.bind_ip.clone(),
            _ => "127.0.0.1".to_string(),
        };

        format!("http://{host}:{}", self.port)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ListJson {
    #[serde(rename = "Common", default)]
    pub common: CommonSection,
    #[serde(rename = "Events", default)]
    pub events: HashMap<String, Vec<[String; 2]>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CommonSection {
    #[serde(rename = "Store", default)]
    pub store: Vec<String>,
    #[serde(rename = "Presentation", default)]
    pub presentation: HashMap<String, Vec<String>>,
    #[serde(rename = "jsonDiffStage", default)]
    pub jsondiff_stage: HashMap<String, Vec<String>>,
    // each key is a label; value has BE (360-only) and LE arrays
    #[serde(flatten)]
    pub extra: HashMap<String, CommonEntry>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CommonEntry {
    #[serde(rename = "BE", default)]
    pub be: Vec<String>, // [label, filename] — empty filename means skip
    #[serde(rename = "LE", default)]
    pub le: Vec<String>, // [label, filename]
}

fn load_json<T: for<'de> serde::Deserialize<'de> + Default>(path: &Path) -> T {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => {
            let text = EMBEDDED_LIST_JSON.trim_start_matches('\u{feff}');
            return serde_json::from_str(text).unwrap_or_else(|e| {
                warn!("Failed to parse embedded content list: {e}");
                T::default()
            });
        }
    };
    let text = text.trim_start_matches('\u{feff}');
    serde_json::from_str(text).unwrap_or_else(|e| {
        warn!("Failed to parse {}: {e}", path.display());
        T::default()
    })
}

// ─── Content list ─────────────────────────────────────────────────────────────

pub struct ContentFile {
    pub id:        u32,
    pub label:     String,
    pub filename:  String,
    pub native_be: bool, // already BE — skip conversion
}

static LABEL_ORDER: &[(&str, u32)] = &[
    ("cooperative", 1),
    ("dictionary", 2),
    ("jsonDiff", 3),
    ("jsonStageDiff", 4),
    ("permission", 5),
    ("presentation", 7),
    ("signboard", 8),
    ("signboard2", 9),
    ("signboardAD02", 10),
    ("signboardAD03", 11),
    ("signboardAD04", 12),
    ("signboardAD05", 13),
    ("signboardAD06", 14),
    ("signboardAD07", 15),
    ("signboardAD08", 16),
    ("signboardAD09", 17),
    ("signboardAD10", 18),
    ("specialCoop", 19)
];

fn label_order(label: &str) -> u32 {
    LABEL_ORDER.iter().find(|(l, _)| *l == label).map(|(_, o)| *o).unwrap_or(999)
}

pub fn required_runtime_paths_for_event(content_dir: &Path, event_id: u32) -> Vec<PathBuf> {
    let text = EMBEDDED_LIST_JSON.trim_start_matches('\u{feff}');
    let list: ListJson = match serde_json::from_str(text) {
        Ok(list) => list,
        Err(_) => return Vec::new(),
    };

    let mut required = Vec::new();
    let eid = event_id.to_string();

    for fname in &list.common.store {
        if !fname.is_empty() {
            required.push(content_dir.join("Common").join("store").join(fname));
        }
    }

    if let Some(file) = resolve_presentation(&list.common.presentation, event_id) {
        required.push(content_dir.join("Common").join("presentation").join(file));
    }

    if let Some(file) = resolve_presentation(&list.common.jsondiff_stage, event_id) {
        required.push(content_dir.join("Common").join("jsonDiffStage").join(file));
    }

    for (label, entry) in &list.common.extra {
        if label == "cooperative" {
            if let Some(fname) = entry.le.get(1).filter(|s| !s.is_empty()) {
                required.push(content_dir.join("Common").join("cooperative").join(fname));
            }
        }
    }

    if let Some(event_files) = list.events.get(&eid) {
        let event_dir = content_dir.join("Events").join(&eid);
        for pair in event_files {
            let fname = &pair[1];
            if !fname.is_empty() {
                required.push(event_dir.join(fname));
            }
        }
    }

    required.sort();
    required.dedup();
    required
}

pub fn build_content_list(
    list: &ListJson,
    project_id: u8,
    event_id: u32,
    id_counter: &mut u32,
) -> Vec<ContentFile> {// gemini = Xbox360
    let is_360 = project_id == 1; 
    let eid_str = event_id.to_string();
    let mut files: Vec<ContentFile> = Vec::new();
    let mut next_id = || { *id_counter += 1; *id_counter };

    for fname in &list.common.store {
        if !fname.is_empty() {
            files.push(ContentFile { id: next_id(), label: "permission".into(), filename: fname.clone(), native_be: false });
        }
    }

    if let Some(pres_file) = resolve_presentation(&list.common.presentation, event_id) {
        files.push(ContentFile { id: next_id(), label: "presentation".into(), filename: pres_file, native_be: false });
    }

    if let Some(stage_file) = resolve_presentation(&list.common.jsondiff_stage, event_id) {
        files.push(ContentFile { id: next_id(), label: "jsonStageDiff".into(), filename: stage_file, native_be: false });
    }

    // Common entries (cooperative, etc.)
    for (label, entry) in &list.common.extra {
        // cooperative BE is only for Xbox360; others: always LE
        if label == "cooperative" {
            if is_360 {
                // BE dedicated to 360
                let fname = entry.be.get(1).map(|s| s.as_str()).unwrap_or("");
                if !fname.is_empty() {
                    files.push(ContentFile { id: next_id(), label: label.clone(), filename: fname.to_string(), native_be: true });
                }
                // empty BE filename → skip entirely (no label, no response)
            } else {
                let fname = entry.le.get(1).map(|s| s.as_str()).unwrap_or("");
                if !fname.is_empty() {
                    files.push(ContentFile { id: next_id(), label: label.clone(), filename: fname.to_string(), native_be: false });
                }
            }
        } else {
            let fname = entry.le.get(1).map(|s| s.as_str()).unwrap_or("");
            if !fname.is_empty() {
                files.push(ContentFile { id: next_id(), label: label.clone(), filename: fname.to_string(), native_be: false });
            }
        }
    }

    if let Some(event_files) = list.events.get(&eid_str) {
        for pair in event_files {
            let label = &pair[0];
            let fname = &pair[1];
            if fname.is_empty() { continue; }
            files.push(ContentFile { id: next_id(), label: label.clone(), filename: fname.clone(), native_be: false });
        }
    }

    files.sort_by_key(|f| label_order(&f.label));
    files
}

fn resolve_presentation(pres: &HashMap<String, Vec<String>>, event_id: u32) -> Option<String> {
    let eid = event_id.to_string();
    pres.iter().find_map(|(filename, list)| {
        if list.contains(&eid) { Some(filename.clone()) } else { None }
    })
}

// ─── jsonDiff merge ───────────────────────────────────────────────────────────

/// Serialize JSON with tab indentation (matching original game file format)
fn to_vec_tab_pretty(value: &serde_json::Value) -> Vec<u8> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut ser).unwrap_or_default();
    buf
}

fn deep_merge(base: serde_json::Value, overlay: serde_json::Value) -> serde_json::Value {
    match (base, overlay) {
        (serde_json::Value::Object(mut b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                let entry = b.entry(k).or_insert(serde_json::Value::Null);
                *entry = deep_merge(entry.take(), v);
            }
            serde_json::Value::Object(b)
        }
        (_, o) => o,
    }
}

fn platform_label(project_id: u8) -> &'static str {
    match project_id {
        1 => "Xbox 360",
        2 => "Xbox One",
        3 => "Win 10",
        _ => "Xbox UWP",
    }
}

fn build_jsondiff_pak(
    index: &FileIndex,
    content_folder: &Path,
    event_id: u32,
    platform: &str,
    _platform_label: &str,
    warnings: &mut Vec<String>,
) -> Option<(Vec<u8>, bool)> {
    let path = find_file_path(index, "Falcon_jsonDiff.pak", Some(platform), Some(event_id))?;
    let raw = fs::read(path).ok()?;

    let raw = if raw.starts_with(b"ZRES") {
        match payload::zres_decompress(&raw) {
            Ok(inner) if inner.starts_with(b"KPK|") => inner,
            _ => raw,
        }
    } else { raw };

    let entries = payload::pak_parse_le(&raw).ok()?;
    let mut entry_map: HashMap<String, payload::PakEntry> =
        entries.iter().cloned().map(|e| (e.name.clone(), e)).collect();
    let entry_order: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();

    let include_base = content_folder.join("include").join("jsonDiff");
    let dirs: &[&str] = if platform == "Xbox360" { &["Global", "360"] } else { &["Global", "One"] };

    let mut extra_order: Vec<String> = Vec::new();
    let mut include_count: usize = 0;

    for dir_name in dirs {
        let dir = include_base.join(dir_name);
        if !dir.is_dir() { continue; }
        let mut files: Vec<_> = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        files.sort();

        for fpath in files {
            if fpath.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let entry_name = fpath.file_stem().unwrap().to_string_lossy().to_string();
            let inc_text = match fs::read_to_string(&fpath) {
                Ok(t) => t,
                Err(e) => { warn!("[include] failed to read \'{}\': {e}", fpath.display()); continue; }
            };
            let inc_data: serde_json::Value = match serde_json::from_str(&inc_text) {
                Ok(v) => v,
                Err(e) => { warnings.push(format!("[include] syntax error in \'{}\': {e} -- file will be skipped", fpath.display())); continue; }
            };

            if let Some(existing) = entry_map.get(&entry_name) {
                let existing_json: serde_json::Value = serde_json::from_slice(&existing.data).unwrap_or_default();
                let merged = deep_merge(existing_json, inc_data);
                let merged_bytes = to_vec_tab_pretty(&merged);
                entry_map.insert(entry_name.clone(), payload::PakEntry {
                    name: entry_name, kind: payload::EntryKind::Zres, data: merged_bytes,
                });
                include_count += 1;
            } else {
                let new_bytes = to_vec_tab_pretty(&inc_data);
                entry_map.insert(entry_name.clone(), payload::PakEntry {
                    name: entry_name.clone(), kind: payload::EntryKind::Zres, data: new_bytes,
                });
                extra_order.push(entry_name);
                include_count += 1;
            }
        }
    }

    let merged_includes = include_count > 0;

    let all_order = entry_order.into_iter().chain(extra_order).collect::<Vec<_>>();
    let merged: Vec<payload::PakEntry> = all_order.into_iter().filter_map(|n| entry_map.remove(&n)).collect();

    let result = if platform == "Xbox360" {
        payload::pak_build_be(&merged)
    } else {
        payload::pak_build_le(&merged)
    };
    Some((result, merged_includes))
}

// ─── File index ───────────────────────────────────────────────────────────────

pub type FileIndex = HashMap<String, Vec<PathBuf>>;

pub fn build_file_index(content_folder: &Path) -> FileIndex {
    let mut index: FileIndex = HashMap::new();
    for entry in walkdir::WalkDir::new(content_folder).into_iter().flatten() {
        if entry.path().components().any(|component| {
            component.as_os_str().to_string_lossy().eq_ignore_ascii_case("Cached")
        }) {
            continue;
        }
        if entry.file_type().is_file() {
            let fname = entry.file_name().to_string_lossy().to_string();
            index.entry(fname).or_default().push(entry.into_path());
        }
    }
    index
}

pub fn find_file_path<'a>(
    index: &'a FileIndex,
    filename: &str,
    platform: Option<&str>,
    stage: Option<u32>,
) -> Option<&'a PathBuf> {
    let paths = index.get(filename)?;
    let stage_s = stage.map(|s| s.to_string());

    if let (Some(p), Some(ref s)) = (platform, &stage_s) {
        if let Some(p) = paths.iter().find(|path| {
            let d = path.to_string_lossy();
            d.contains(p) && d.contains(s.as_str())
        }) { return Some(p); }
    }
    if let Some(ref s) = stage_s {
        if let Some(p) = paths.iter().find(|path| path.to_string_lossy().contains(s.as_str())) {
            return Some(p);
        }
    }
    if let Some(p) = platform {
        if let Some(path) = paths.iter().find(|path| path.to_string_lossy().contains(p)) {
            return Some(path);
        }
    }
    paths.first()
}

// ─── CDN token ────────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn md5_hex(s: &str) -> String {
    use md5::{Md5, Digest};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

pub fn generate_cdn_token(label: &str) -> (String, u64) {
    let expiry = now_secs() + 300;
    let c = &md5_hex(&format!("{label}{expiry}"))[..16];
    (c.to_string(), expiry)
}

// ─── File info ────────────────────────────────────────────────────────────────

pub struct FileInfo { pub size: u64, pub hash: String }

pub fn compute_file_info(data: &[u8]) -> FileInfo {
    let mut hasher = Sha256::new();
    hasher.update(data);
    FileInfo { size: data.len() as u64, hash: hex::encode(hasher.finalize()) }
}

// ─── Cache ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CacheEntry {
    pub data:     Vec<u8>,
    pub filename: String,
    pub mtime:    u64,
}

// ─── LE→BE for gemini ─────────────────────────────────────────────────────────

static SKIP_EXTS: &[&str] = &["jpg", "json"];

fn convert_to_be(data: Vec<u8>, filename: &str) -> Result<Vec<u8>, String> {
    let ext = Path::new(filename).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if SKIP_EXTS.contains(&ext.as_str()) { return Ok(data); }

    let result = payload::pak_convert_to_360(&data);
    if let Err(ref e) = result {
        warn!("[Gemini] conversion error for {filename}: {e}");
    }
    result
}

// ─── AppState ─────────────────────────────────────────────────────────────────

pub struct AppState {
    pub config:          Config,
    pub list:            ListJson,
    pub file_index:      RwLock<FileIndex>,
    pub file_cache:      RwLock<HashMap<String, CacheEntry>>,
    pub cache_event_ids: RwLock<HashMap<u8, u32>>,
}

impl AppState {
    pub async fn init_with_config(config: Config) -> (Self, Vec<String>) {
        let content_folder = config.content_folder();
        let list: ListJson = load_json(&content_folder.join("database.json"));

        let file_index = build_file_index(&content_folder);
        let mut id_counter = 10000u32;
        let (file_cache, cache_event_ids, warnings) = build_cache(&config, &list, &file_index, &mut id_counter);

        (Self { config, list, file_index: RwLock::new(file_index), file_cache: RwLock::new(file_cache), cache_event_ids: RwLock::new(cache_event_ids) }, warnings)
    }

    pub fn reload(&self) {
        let cf = self.config.content_folder();
        let new_index = build_file_index(&cf);
        let mut id_counter = 10000u32;
        let (new_cache, new_eids, _) = build_cache(&self.config, &self.list, &new_index, &mut id_counter);
        *self.file_index.write() = new_index;
        *self.file_cache.write() = new_cache;
        *self.cache_event_ids.write() = new_eids;
        info!("[cdn] cache reloaded");
    }

    pub fn events_changed(&self) -> bool {
        let eids = self.cache_event_ids.read();
        let cfg = &self.config;
        *eids != [(1u8, cfg.current_event.xbox360), (2, cfg.current_event.xbox_uwp), (3, cfg.current_event.xbox_uwp)]
            .iter().cloned().collect::<HashMap<_, _>>()
    }
}

fn build_cache(
    config: &Config,
    list: &ListJson,
    index: &FileIndex,
    id_counter: &mut u32,
) -> (HashMap<String, CacheEntry>, HashMap<u8, u32>, Vec<String>) {
    let cf = config.content_folder();

    let mut cache: HashMap<String, CacheEntry> = HashMap::new();
    let mut event_ids: HashMap<u8, u32> = HashMap::new();
    let mut syntax_warnings: Vec<String> = Vec::new();
    let mut jsondiff_jobs: Vec<(u32, &'static str, String, String, ContentFile)> = Vec::new();
    let mut regular_jobs: Vec<(u32, &'static str, String, bool, ContentFile)> = Vec::new();

    for (project_id, prefix) in [(1u8, "gemini"), (2, "falcon"), (3, "castor")] {
        let event_id = config.event_id(project_id);
        event_ids.insert(project_id, event_id);

        let platform = if project_id == 1 { "Xbox360" } else { "XboxUWP" };
        let is_360 = project_id == 1;

        let files = build_content_list(list, project_id, event_id, id_counter);

        for cf_entry in files {
            let key = format!("{prefix}/{}", cf_entry.label);
            if cache.contains_key(&key) { continue; }

            if cf_entry.label == "jsonDiff" {
                jsondiff_jobs.push((event_id, platform, platform_label(project_id).to_string(), key, cf_entry));
                continue;
            }

            regular_jobs.push((event_id, platform, key, is_360, cf_entry));
        }
    }

    let mut merged_jsondiff = false;
    for (event_id, platform, platform_name, key, cf_entry) in jsondiff_jobs {
        match build_jsondiff_pak(index, &cf, event_id, platform, &platform_name, &mut syntax_warnings) {
            Some((data, merged_includes)) => {
                merged_jsondiff |= merged_includes;
                cache.insert(key, CacheEntry { data, filename: cf_entry.filename, mtime: now_secs() });
            }
            None => warn!("[cache] jsondiff build failed event={event_id} platform={platform_name}"),
        }
    }
    if merged_jsondiff {
        info!("[Include] Merging into jsonDiff");
    }

    let mut converted_to_360 = false;
    for (event_id, platform, key, is_360, cf_entry) in regular_jobs {
        let fpath = match find_file_path(index, &cf_entry.filename, Some(platform), Some(event_id)) {
            Some(p) => p.clone(),
            None => { warn!("[cache] not found: {}", cf_entry.filename); continue; }
        };

        let data = match fs::read(&fpath) {
            Ok(d) => d,
            Err(e) => { warn!("[cache] read error {}: {e}", fpath.display()); continue; }
        };

        let mtime = fpath.metadata()
            .and_then(|m| m.modified())
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
            .unwrap_or(0);

        let ext = fpath.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let data = if is_360 && !cf_entry.native_be && !SKIP_EXTS.contains(&ext.as_str()) {
            match convert_to_be(data, &cf_entry.filename) {
                Ok(converted) => { converted_to_360 = true; converted }
                Err(_) => continue,
            }
        } else {
            data
        };

        cache.insert(key, CacheEntry { data, filename: cf_entry.filename, mtime });
    }
    if converted_to_360 {
        info!("[Gemini] Converting files to Xbox 360");
    }

    (cache, event_ids, syntax_warnings)
}

// ─── CDN HTTP handler ─────────────────────────────────────────────────────────

pub async fn cdn_handler(
    AxumPath(label): AxumPath<String>,
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<Body>,
) -> Response<Body> {
    // Derive prefix from the request URI (e.g. "/falcon/jsonDiff" → "falcon")
    let uri_prefix = req.uri().path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();

    if state.events_changed() { state.reload(); }

    let cache = state.file_cache.read();
    // Try the exact prefix from the URL first, then fall back to others
    let mut found_key: Option<String> = None;
    let fallback_order = ["gemini", "falcon", "castor"];
    let ordered: Vec<&str> = std::iter::once(uri_prefix.as_str())
        .chain(fallback_order.iter().copied().filter(|&p| p != uri_prefix.as_str()))
        .collect();
    for p in ordered {
        let key = format!("{p}/{label}");
        if cache.contains_key(&key) { found_key = Some(key); break; }
    }

    let key = match found_key {
        Some(k) => k,
        None => return Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap(),
    };

    let entry = match cache.get(&key) {
        Some(e) => e,
        None => return Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap(),
    };

    let data = entry.data.clone();
    let mtime = entry.mtime;
    let size = data.len();

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", size.to_string())
        .header("Connection", "close")
        .header("Accept-Ranges", "bytes")
        .header("ETag", format!("\"{mtime:x}-{size:x}\""))
        .header("Last-Modified", format_http_date(mtime))
        .body(Body::from(data))
        .unwrap()
}

fn format_http_date(secs: u64) -> String {
    use chrono::{TimeZone, Utc};
    let dt = Utc.timestamp_opt(secs as i64, 0).unwrap();
    dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}