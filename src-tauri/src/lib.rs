use std::path::PathBuf;

use compliancy_core::{
    AssistConfig, AssistRuntime, AssistStatus, RunRequest, RunResult, TemplatePreview,
};

#[tauri::command]
fn get_assist_status(mode: String, assist: Option<AssistConfig>) -> AssistStatus {
    let mode = if mode.eq_ignore_ascii_case("assist") {
        "assist"
    } else {
        "standard"
    };
    AssistRuntime::for_mode(mode, assist).status
}

#[tauri::command]
fn run_pipeline(request: RunRequest) -> RunResult {
    compliancy_core::run_pipeline(request)
}

/// Partially load a master template: detect fonts / colours / backgrounds for the picker.
#[tauri::command]
fn inspect_template(path: String) -> Result<TemplatePreview, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("template not found: {path}"));
    }
    compliancy_core::inspect_template(&p).map_err(|e| e.to_string())
}

#[tauri::command]
fn app_info() -> serde_json::Value {
    serde_json::json!({
        "name": "Compliancy Checker",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["standard", "assist"],
        "formats": ["docx"],
        "isoStandards": ["9001", "14001", "45001"],
        "features": [
            "docx template style merge",
            "template trait preview + picker",
            "ISO section checklist export",
            "structured context rules",
            "Assist OpenAI-compatible LLM hook"
        ],
        "notes": {
            "pdf": "PDF ingest/export tracked separately (CC-11) — not in this release"
        },
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            app_info,
            get_assist_status,
            inspect_template,
            run_pipeline
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
