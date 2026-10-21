//! Static consistency checks for the browser assets. These catch the class of
//! regression that only shows up in a real browser (stale cache-busting
//! versions, Tabulator column-field collisions, theme load order).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn templates() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(repo().join("templates")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".html") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        out.push((name, text));
    }
    out
}

fn versions(text: &str, asset: &str) -> BTreeSet<u32> {
    let needle = format!("{asset}?v=");
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(idx) = rest.find(&needle) {
        let after = &rest[idx + needle.len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(version) = digits.parse() {
            out.insert(version);
        }
        rest = after;
    }
    out
}

#[test]
fn asset_cache_busting_versions_are_consistent_across_templates() {
    for asset in ["compare.js", "style.css", "tabulator.min.css"] {
        let mut seen = BTreeSet::new();
        for (name, text) in templates() {
            let found = versions(&text, asset);
            assert!(
                found.len() <= 1,
                "{name} references {asset} with conflicting versions: {found:?}"
            );
            seen.extend(found);
        }
        assert_eq!(
            seen.len(),
            1,
            "{asset} must use exactly one cache-busting version across templates, saw {seen:?}"
        );
    }
}

#[test]
fn referenced_static_assets_exist() {
    for (name, text) in templates() {
        let mut rest = text.as_str();
        while let Some(idx) = rest.find("/static/") {
            let after = &rest[idx..];
            let end = after
                .find(|c: char| c == '"' || c == '\'' || c == '?' || c == ' ' || c == '\n')
                .unwrap_or(after.len());
            let rel = &after[..end];
            if !rel.contains('{') {
                let path = repo().join(rel.trim_start_matches('/'));
                assert!(path.exists(), "{name} references missing asset {rel}");
            }
            rest = &after[end.max(1)..];
        }
    }
}

#[test]
fn vendor_theme_loads_before_the_app_stylesheet() {
    let base = read("templates/base.html");
    let vendor = base
        .find("tabulator.min.css")
        .expect("vendor theme linked from base.html");
    let app = base
        .find("style.css")
        .expect("app stylesheet linked from base.html");
    assert!(
        vendor < app,
        "tabulator.min.css must load before style.css or it overrides the dark theme"
    );
}

#[test]
fn preview_columns_use_unique_fields_and_the_formatter_is_idempotent() {
    let js = read("static/compare.js");
    let previews: Vec<&str> = js
        .lines()
        .filter(|line| line.contains("title: \"Preview\""))
        .collect();

    assert_eq!(previews.len(), 2, "groups + matches tables each have a preview column");
    for line in previews {
        assert!(
            line.contains("field: \"preview\""),
            "preview columns need a unique field (shared fields made persisted layouts duplicate cells): {line}"
        );
        assert!(
            !line.contains("field: \"keeper\"") && !line.contains("field: \"a\""),
            "preview must not share a field with a path column: {line}"
        );
    }

    assert!(
        js.contains("cellEl.querySelectorAll(\".previews\")"),
        "previewFormatter must remove a previous preview block before rendering"
    );
    assert!(
        js.contains("window.__initSimilarTables = initSimilarTables;"),
        "the SSE done handler needs the global init hook"
    );
}

#[test]
fn organize_result_page_explains_truncated_rows() {
    let html = read("templates/organize_result.html");
    assert!(
        html.contains("rows_more") && html.contains("more not shown"),
        "truncated tables must say how many rows were omitted"
    );
}
