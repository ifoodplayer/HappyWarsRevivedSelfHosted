use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use axum::{
    body::Body,
    extract::{Request, State},
    response::Response,
};
use tracing::info;
use urlencoding::encode as urlencode;
use indexmap::IndexMap;

use crate::api::cdn::{
    build_content_list, compute_file_info, find_file_path, generate_cdn_token, AppState,
};

// ─── Route config ─────────────────────────────────────────────────────────────

struct RouteInfo {
    allowed_pairs: &'static [(u8, u8)],
}

fn route_info(path: &str) -> Option<RouteInfo> {
    match path {
        "/api" => Some(RouteInfo {
            allowed_pairs: &[(1, 1)],
        }),
        "/v12/api" => Some(RouteInfo {
            allowed_pairs: &[(2, 2), (3, 3)],
        }),
        _ => None,
    }
}

const PROJECT_PREFIX: &[(u8, &str)] = &[(1, "gemini"), (2, "falcon"), (3, "castor")];

fn prefix_for(project_id: u8) -> &'static str {
    PROJECT_PREFIX.iter().find(|(p, _)| *p == project_id).map(|(_, s)| *s).unwrap_or("content")
}

const VALID_SECONDS: u64 = 300;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn json_response(body: String, status: u16) -> Response<Body> {
    let bytes = body.into_bytes();
    let len = bytes.len();
    Response::builder()
        .status(status)
        .header("Cache-Control", "no-cache, private")
        .header("Content-Type", "application/json; charset=UTF-8")
        .header("Connection", "close")
        .header("Content-Length", len.to_string())
        .body(Body::from(bytes))
        .unwrap()
}

fn not_authorized()    -> Response<Body> { json_response(r#"{"error":"Not authorized"}"#.into(), 200) }
fn missing_parameters() -> Response<Body> { json_response(r#"{"error":"missing parameters"}"#.into(), 200) }

fn vulcan_minimal(version: u32) -> Response<Body> {
    json_response(format!(r#"{{"Format":{{"Name":"Vulcan","Version":{version}}}}}"#), 200)
}

// ─── Handler ──────────────────────────────────────────────────────────────────

pub async fn xas_handler(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response<Body> {
    let path = req.uri().path().to_string();
    let headers = req.headers().clone();
    let query_str = req.uri().query().unwrap_or("").to_string();

    let params: HashMap<String, String> = form_urlencoded::parse(query_str.as_bytes())
        .into_owned().collect();

    let info = match route_info(&path) {
        Some(i) => i,
        None    => return not_authorized(),
    };

    let required = ["country_id", "language_id", "platform_id", "project_id", "stage_id", "version"];
    if required.iter().any(|k| !params.contains_key(*k)) {
        return missing_parameters();
    }

    let platform_id: u8 = match params["platform_id"].parse() { Ok(v) => v, Err(_) => return not_authorized() };
    let project_id:  u8 = match params["project_id"].parse()  { Ok(v) => v, Err(_) => return not_authorized() };
    let stage_id:    u8 = match params["stage_id"].parse()    { Ok(v) => v, Err(_) => return not_authorized() };

    let is_valid_pair = info.allowed_pairs.iter().any(|(allowed_platform, allowed_project)| {
        *allowed_platform == platform_id && *allowed_project == project_id
    });

    if !is_valid_pair || stage_id != 1 {
        return not_authorized();
    }

    let xbox_user = headers.get("x-id-hash")
        .and_then(|h| h.to_str().ok()).unwrap_or("unknown").to_uppercase();

    let platform_name = match platform_id {
        1 => "Xbox 360",
        2 => "Xbox One",
        3 => "Windows 10",
        _ => "Unknown",
    };

    info!("[User] XUIDHash={xbox_user}. Platform = {platform_name}");

    let event_id = state.config.event_id(project_id);
    let mut id_counter = 10000u32;
    let files = build_content_list(&state.list, project_id, event_id, &mut id_counter);

    let version: u32 = path.trim_start_matches('/')
        .strip_prefix('v')
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(9);

    if files.is_empty() { return vulcan_minimal(version); }

    let current_time = now_secs();
    let project_prefix = prefix_for(project_id);
    let cdn_host = state.config.cdn_base_url_for_project(project_id);

    let file_index = state.file_index.read();
    let file_cache = state.file_cache.read();
    let mut file_list: IndexMap<String, serde_json::Value> = IndexMap::new();

    for (file_idx, cf_entry) in files.iter().enumerate() {
        let key = format!("{project_prefix}/{}", cf_entry.label);

        let (size, hash) = if let Some(entry) = file_cache.get(&key) {
            let info = compute_file_info(&entry.data);
            (info.size, info.hash)
        } else {
            let platform = if project_id == 1 { "Xbox360" } else { "XboxUWP" };
            match find_file_path(&file_index, &cf_entry.filename, Some(platform), Some(event_id)) {
                Some(p) => match std::fs::read(p) {
                    Ok(data) => { let info = compute_file_info(&data); (info.size, info.hash) }
                    Err(_) => continue,
                },
                None => continue,
            }
        };

        let (c, expiry) = generate_cdn_token(&cf_entry.label);
        let raw_url = format!("{cdn_host}/{project_prefix}/{}?c={c}&e={expiry}", cf_entry.label);
        let url = urlencode(&raw_url).into_owned();

        file_list.insert(
            (file_idx + 1).to_string(),
            serde_json::json!({
                "AssetName": "SelfHosted",
                "Label":     cf_entry.label,
                "URL":       url,
                "FileName":  cf_entry.filename,
                "FileSize":  size,
                "Memo":      "",
                "FileHash":  hash,
                "Addition":  "",
                "Id":        cf_entry.id,
            }),
        );
    }

    let reward = &state.config.reward;

    let mut response_map = indexmap::IndexMap::new();
    response_map.insert("Format", serde_json::json!({"Name": "Vulcan", "Version": version}));
    response_map.insert("List",   serde_json::json!({"Name": "SelfHosted", "Valid": current_time + VALID_SECONDS, "Memo": "Customized", "Id": event_id}));
    response_map.insert("File",   serde_json::json!(file_list));

    if reward.enabled {
        let extras_key = {
            let ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            format!("{:05}", ns % 100_000)
        };
        let extras_val = serde_json::json!({
            "TargetProject": {
                "ProjectId_1": {"Id": 1},
                "ProjectId_2": {"Id": 2},
                "ProjectId_3": {"Id": 3},
            },
            "XUIDHash":  xbox_user,
            "EndDate":   reward.end_date,
            "TicketNum": reward.ticket_num,
            "TXID_01":   reward.txid_01,
            "RewardSet": reward.reward_set
        });
        response_map.insert("Extras", serde_json::json!({extras_key: extras_val}));
    }

    json_response(serde_json::to_string(&response_map).unwrap(), 200)
}