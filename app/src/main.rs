#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod api;

use axum::{
    extract::Request,
    middleware::{self, Next},
    response::Response,
    Router,
    routing::{get, post},
};
use std::{fs, path::PathBuf, sync::Arc};
use tracing::{info, warn};

const DEFAULT_SUNRISE2_INI: &str = include_str!("config/sunrise2.ini");

pub const SERVER_PORT: u16 = 8354;

pub use api::cdn::AppState;

#[cfg(target_os = "windows")]
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("\n[PANIC] {info}");
        eprintln!("Press Enter to close...");
        let _ = std::io::Read::read(&mut std::io::stdin(), &mut [0u8]);
    }));
}

#[cfg(not(target_os = "windows"))]
fn install_panic_hook() {}

#[cfg(target_os = "windows")]
fn setup_console() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    unsafe {
        windows_sys::Win32::System::Console::AllocConsole();

        use windows_sys::Win32::System::Console::{
            GetConsoleMode, SetConsoleMode, GetStdHandle,
            ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE,
        };

        for handle_id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(handle_id);
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) != 0 {
                mode &= !(ENABLE_QUICK_EDIT_MODE as u32);
                if handle_id == STD_OUTPUT_HANDLE || handle_id == STD_ERROR_HANDLE {
                    mode |= ENABLE_VIRTUAL_TERMINAL_PROCESSING;
                }
                SetConsoleMode(handle, mode);
            }
        }

        let title: Vec<u16> = OsStr::new("Happy Wars Revived - Self Hosted")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        windows_sys::Win32::System::Console::SetConsoleTitleW(title.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
fn setup_console() {}

fn local_ipv4() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn runtime_ini_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn render_ini_with_host(template: &str, host: &str) -> String {
    template
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("Host=") {
                format!("Host={host}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ensure_runtime_sidecar_ini(file_name: &str, template: &str, host: &str) {
    let path = runtime_ini_dir().join(file_name);
    let rendered = render_ini_with_host(template, host);

    match fs::read_to_string(&path) {
        Ok(existing) => {
            let host_matches = existing
                .lines()
                .any(|line| {
                    let trimmed = line.trim_start();
                    trimmed.starts_with("Host=")
                        && trimmed
                            .split_once('=')
                            .map(|(_, value)| value.trim())
                            .unwrap_or("")
                            == host
                });

            if !host_matches {
                if let Err(err) = fs::write(&path, &rendered) {
                    warn!("[Config] Could not regenerate {}: {err}", file_name);
                } else {
                    warn!("[Config] Regenerating {}", file_name);
                }
            }
        }
        Err(_) => {
            if let Err(err) = fs::write(&path, rendered) {
                warn!("[Config] Could not generate {}: {err}", file_name);
            } else {
                warn!("[Config] Generating {}", file_name);
            }
        }
    }
}

fn ensure_required_runtime_files(config: &api::cdn::Config) -> Result<(), String> {
    let content_dir = std::path::Path::new("content");

    if !content_dir.is_dir() {
        return Err(format!("Content folder not found: {}", std::env::current_dir().unwrap_or_default().join("content").display()));
    }

    for event_id in [config.current_event.xbox360, config.current_event.xbox_uwp] {
        let path = content_dir.join("Events").join(event_id.to_string());
        if !path.is_dir() {
            return Err(format!("Event {} file not found: {}. Exiting.", event_id, path.display()));
        }

        let has_files = std::fs::read_dir(&path)
            .map(|entries| entries.count() > 0)
            .unwrap_or(false);

        if !has_files {
            return Err(format!("Event {} folder is empty: {}. Exiting.", event_id, path.display()));
        }

        for required_path in api::cdn::required_runtime_paths_for_event(content_dir, event_id) {
            if !required_path.is_file() {
                return Err(format!("Event {} file not found: {}. Exiting.", event_id, required_path.display()));
            }
        }
    }

    Ok(())
}

async fn request_logger(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let full_path = match req.uri().query() {
        Some(q) => format!("{}?{}", path, q),
        None    => path.clone(),
    };

    let is_cmsresource = path == "/cmsresource/HappyWars";
    let response = next.run(req).await;
    let status = response.status().as_u16();

    if path == "/cmsresource/HappyWars" {
        warn!("[Resource] {method} {full_path} - {status}");
    } else if path.to_ascii_lowercase().contains("/api/telemetryentity") {
        warn!("[Telemetry] {method} {full_path} - {status}");
    } else if !is_cmsresource {
        let tag = if path == "/api" || path == "/v12/api"
            || path.starts_with("/gemini/")
            || path.starts_with("/falcon/")
            || path.starts_with("/castor/") { "[API] " }
        else if path.starts_with("/Services.Sentient/") { "[XLSP] " }
        else if path.starts_with("/title/") { "[Xenia] " }
        else { "" };
        info!("{tag}{method} {full_path} - {status}");
    }

    response
}

#[tokio::main]
async fn main() {
    setup_console();
    install_panic_hook();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(true)
        .with_target(false)
        .without_time()
        .compact()
        .init();

    let bind_ip = match local_ipv4() {
        Some(ip) => ip.to_string(),
        None => {
            tracing::warn!("Unable to detect IPv4. Are you connected to Internet?");
            "0.0.0.0".into()
        }
    };

    let config = api::cdn::Config::load("config.ini").with_bind_ip(bind_ip.clone());
    ensure_runtime_sidecar_ini("Sunrise2.ini", DEFAULT_SUNRISE2_INI, &bind_ip);
    let port = config.port;

    if let Err(err) = ensure_required_runtime_files(&config) {
        eprintln!(" \x1b[33mWARN\x1b[0m ******************************");
        eprintln!(" \x1b[31mERROR\x1b[0m {err}");
        eprintln!(" \x1b[33mWARN\x1b[0m ******************************");
        eprintln!("");
        eprintln!(" \x1b[33mINFO\x1b[0m Press Enter to close...");
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        std::process::exit(1);
    }

    let (app_state, startup_warnings) = AppState::init_with_config(config).await;
    let state = Arc::new(app_state);

    let app = Router::new()
        .route("/api",     get(api::xas::xas_handler))
        .route("/v12/api", get(api::xas::xas_handler))
        .route("/gemini/:label", get(api::cdn::cdn_handler))
        .route("/falcon/:label", get(api::cdn::cdn_handler))
        .route("/castor/:label", get(api::cdn::cdn_handler))
        .route("/title/58410AE9/servers", get(api::legacy::xenia_servers))
        .route("/title/58410AE9/ports",   get(api::legacy::xenia_ports))
        .route("/cmsresource/HappyWars",  get(api::legacy::resource))
        .route("/Services.Sentient/o.rhbin/Microsoft.Games.Sentient.DynamicConfigService,DynamicConfigService,Version=1.0.2",  post(api::legacy::handle_sentient))
        .route("/Services.Sentient/o.rhbin/Rhino.Services.Platform,Rhino.Services.Platform,PlatformService,Version=1.0.2",     post(api::legacy::handle_sentient))
        .route("/Services.Sentient/o.rhbin/Rhino.Services.Platform,PlatformService,Version=1.0.2",                             post(api::legacy::handle_sentient))
        .route("/Services.Sentient/o.rhbin/Microsoft.Games.Sentient.NewsService,NewsService,Version=1.0.2",                    post(api::legacy::handle_sentient))
        .route("/Services.Sentient/o.rhbin/Microsoft.Games.Sentient.TelemetryService,TelemetryService,Version=1.0.2",          post(api::legacy::handle_sentient))
        .layer(middleware::from_fn(request_logger))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    for w in &startup_warnings {
        tracing::warn!("{w}");
    }

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!(" \x1b[33mWARN\x1b[0m ****************************************************");
            eprintln!(" \x1b[31mERROR\x1b[0m Port {port} It is already in use.");
            eprintln!(" \x1b[31mERROR\x1b[0m Close whatever is using port {port} and try again.");
            eprintln!(" \x1b[31mERROR\x1b[0m Details: {err}");
            eprintln!(" \x1b[33mWARN\x1b[0m ****************************************************");
            eprintln!("");
            eprintln!(" \x1b[33mINFO\x1b[0m Press Enter to close...");
            let mut input = String::new();
            let _ = std::io::stdin().read_line(&mut input);
            std::process::exit(1);
        }
    };

    info!("[Boot] Server listening on {} (IPv4 local: {})", addr, bind_ip);
    axum::serve(listener, app).await.unwrap();
}