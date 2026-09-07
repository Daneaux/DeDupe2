//! Photo Tools – a separate web server (port 8090) with two tabs:
//!   1. Folder Dedupe
//!   2. Find Original Images
//!
//! Run with:  cargo run --bin photo_tools

use std::path::Path;

use axum::{
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
use askama_axum::Template;
use serde::{Deserialize, Serialize};
use tower_http::{
    cors::CorsLayer,
    services::ServeFile,
    trace::TraceLayer,
};
use tracing::info;

use dedupe2::exif::creation_date;
use dedupe2::filemover::{find_duplicates, merge_dirs, DuplicateStrategy, Operation};
use dedupe2::image_reader::hash_all_bytes;

// ── Templates ──────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "photo_tools/dedupe.html", ext = "html")]
struct DedupeTemplate {
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "photo_tools/originals.html", ext = "html")]
struct OriginalsTemplate {
    active_tab: &'static str,
}

// ── Page handlers ──────────────────────────────────────────────────────────

async fn root() -> Redirect {
    Redirect::to("/photo_tools/dedupe")
}

async fn dedupe_page() -> DedupeTemplate {
    DedupeTemplate { active_tab: "dedupe" }
}

async fn originals_page() -> OriginalsTemplate {
    OriginalsTemplate { active_tab: "originals" }
}

// ── API: Folder Dedupe ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DedupeRequest {
    folder_a: String,
    folder_b: String,
    dst: String,
    strategy: String, // "exact" | "image"
    op: String,       // "move" | "copy"
}

#[derive(Serialize)]
struct FileEntry {
    path: String,
    size: u64,
}

#[derive(Serialize)]
struct DupGroup {
    hash: String,
    files: Vec<FileEntry>,
}

#[derive(Serialize)]
struct FindDupesResponse {
    total_groups: usize,
    total_files: usize,
    groups: Vec<DupGroup>,
}

async fn api_find_duplicates(
    Json(req): Json<DedupeRequest>,
) -> Result<Json<FindDupesResponse>, (StatusCode, Json<serde_json::Value>)> {
    let strategy = match req.strategy.as_str() {
        "image" => DuplicateStrategy::ImageData,
        _ => DuplicateStrategy::ExactHash,
    };

    let folder_a = Path::new(&req.folder_a);
    let folder_b = Path::new(&req.folder_b);

    if !folder_a.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Folder A not found: {}", req.folder_a) })),
        ));
    }
    if !folder_b.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Folder B not found: {}", req.folder_b) })),
        ));
    }

    // Collect files from both folders (shallow)
    let mut all_files: Vec<std::path::PathBuf> = Vec::new();
    for dir in [folder_a, folder_b] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    all_files.push(entry.path());
                }
            }
        }
    }

    let groups = find_duplicates(&all_files, strategy).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;

    let total_files: usize = groups.iter().map(|g| g.len()).sum();

    let resp_groups: Vec<DupGroup> = groups
        .iter()
        .map(|group| {
            let first = &group[0];
            let hash = hash_all_bytes(first)
                .map(|h| format!("{:016x}", h))
                .unwrap_or_else(|_| "unknown".into());

            let files: Vec<FileEntry> = group
                .iter()
                .map(|p| {
                    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                    FileEntry {
                        path: p.display().to_string(),
                        size,
                    }
                })
                .collect();

            DupGroup { hash, files }
        })
        .collect();

    let total_groups = resp_groups.len();

    Ok(Json(FindDupesResponse {
        total_groups,
        total_files,
        groups: resp_groups,
    }))
}

#[derive(Serialize)]
struct MergeResponse {
    moved_files: usize,
    discarded: usize,
}

async fn api_merge(
    Json(req): Json<DedupeRequest>,
) -> Result<Json<MergeResponse>, (StatusCode, Json<serde_json::Value>)> {
    let strategy = match req.strategy.as_str() {
        "image" => DuplicateStrategy::ImageData,
        _ => DuplicateStrategy::ExactHash,
    };
    let op = match req.op.as_str() {
        "copy" => Operation::Copy,
        _ => Operation::Move,
    };

    let folder_a = Path::new(&req.folder_a).to_path_buf();
    let folder_b = Path::new(&req.folder_b).to_path_buf();
    let dst = Path::new(&req.dst).to_path_buf();

    for (name, p) in [("Folder A", &folder_a), ("Folder B", &folder_b)] {
        if !p.is_dir() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("{name} not found: {}", p.display()) })),
            ));
        }
    }

    let before = count_files(&folder_a) + count_files(&folder_b);

    merge_dirs(&folder_a, &folder_b, &dst, op, strategy).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;

    let after = count_files(&dst);
    let moved_files = after;
    let discarded = before.saturating_sub(after);

    Ok(Json(MergeResponse { moved_files, discarded }))
}

fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok()).filter(|e| e.path().is_file()).count())
        .unwrap_or(0)
}

// ── API: Find Original Images ─────────────────────────────────────────────

#[derive(Deserialize)]
struct OriginalsRequest {
    folder: String,
    mode: String, // "exif" | "name"
}

const RAW_EXTS: &[&str] = &[
    "raw", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "rw2",
    "orf", "pef", "raf", "dng", "srw", "x3f", "iiq", "erf", "3fr", "kdc",
    "dcr", "dcs", "mef", "mos", "mrw", "rwl", "fff", "bay", "ari",
];

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "ico",
    "pnm", "pbm", "pgm", "ppm", "pam", "qoi", "avif", "hdr", "exr", "heic",
    "heif", "psd", "psb",
];

#[derive(Serialize, Clone)]
struct OrigFile {
    name: String,
    path: String,
    size: u64,
    date: String,
    #[serde(rename = "isRaw")]
    is_raw: bool,
}

#[derive(Serialize)]
struct OrigGroup {
    date: String,
    files: Vec<OrigFile>,
    original: Option<OrigFile>,
}

#[derive(Serialize)]
struct OriginalsResponse {
    total_files: usize,
    groups: Vec<OrigGroup>,
}

async fn api_find_originals(
    Json(req): Json<OriginalsRequest>,
) -> Result<Json<OriginalsResponse>, (StatusCode, Json<serde_json::Value>)> {
    let folder = Path::new(&req.folder);
    if !folder.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Folder not found: {}", req.folder) })),
        ));
    }

    let mode_exif = req.mode == "exif";

    // Walk the directory
    let mut files: Vec<OrigFile> = Vec::new();

    for entry in walkdir::WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let is_raw = RAW_EXTS.contains(&ext.as_str());
        let is_image = IMAGE_EXTS.contains(&ext.as_str()) || is_raw;
        if !is_image {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let date = creation_date(path).value().unwrap_or("").to_string();

        files.push(OrigFile {
            path: path.display().to_string(),
            is_raw,
            name,
            size,
            date,
        });
    }

    // Capture count before `files` is consumed by the grouping loops below
    let total_files = files.len();

    // Group files
    let mut groups: Vec<OrigGroup> = Vec::new();

    if mode_exif {
        // Group by creation date (date portion only)
        let mut by_date: std::collections::HashMap<String, Vec<OrigFile>> =
            std::collections::HashMap::new();
        for f in files {
            let key = if f.date.is_empty() {
                "unknown".to_string()
            } else {
                f.date.split(' ').next().unwrap_or(&f.date).to_string()
            };
            by_date.entry(key).or_default().push(f);
        }

        for (date, mut group_files) in by_date {
            group_files.sort_by(|a, b| {
                b.is_raw.cmp(&a.is_raw).then(b.size.cmp(&a.size))
            });

            let original = group_files.iter().find(|f| f.is_raw).cloned();
            groups.push(OrigGroup {
                date,
                files: group_files,
                original,
            });
        }
    } else {
        // Group by filename stem (strip extension)
        let mut by_name: std::collections::HashMap<String, Vec<OrigFile>> =
            std::collections::HashMap::new();
        for f in files {
            let stem = f
                .name
                .rsplit('.')
                .next()
                .unwrap_or(&f.name)
                .to_lowercase();
            by_name.entry(stem).or_default().push(f);
        }

        for (_stem, mut group_files) in by_name {
            group_files.sort_by(|a, b| {
                b.is_raw.cmp(&a.is_raw).then(b.size.cmp(&a.size))
            });

            let original = group_files.iter().find(|f| f.is_raw).cloned();
            let date = group_files
                .iter()
                .find(|f| !f.date.is_empty())
                .map(|f| f.date.clone())
                .unwrap_or_default();

            groups.push(OrigGroup {
                date,
                files: group_files,
                original,
            });
        }
    }

    groups.sort_by(|a, b| b.date.cmp(&a.date));

    Ok(Json(OriginalsResponse {
        total_files,
        groups,
    }))
}

// ── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "photo_tools=debug,axum=info,tower_http=info".into()),
        )
        .init();

    let port: u16 = std::env::var("PHOTO_TOOLS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8090);

    let app = Router::new()
        // Redirect root
        .route("/photo_tools", get(root))
        // Pages
        .route("/photo_tools/dedupe", get(dedupe_page))
        .route("/photo_tools/originals", get(originals_page))
        // API
        .route("/photo_tools/api/find_duplicates", post(api_find_duplicates))
        .route("/photo_tools/api/merge", post(api_merge))
        .route("/photo_tools/api/find_originals", post(api_find_originals))
        // Static assets (ServeFile is a tower Service, use route_service)
        .route_service(
            "/photo_tools/style.css",
            ServeFile::new("static/photo_tools/style.css"),
        )
        .route_service(
            "/photo_tools/app.js",
            ServeFile::new("static/photo_tools/app.js"),
        )
        .layer(TraceLayer::new_for_http())
        // Local dev tool – allow all origins
        .layer(CorsLayer::permissive());

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    info!(
        "Photo Tools server listening on http://{addr}  (dedupe: /photo_tools/dedupe, originals: /photo_tools/originals)"
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    axum::serve(listener, app)
        .await
        .expect("server error");
}
