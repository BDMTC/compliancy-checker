mod assist;
mod pipeline;

use assist::{AssistConfig, AssistStatus, StubLlmProvider};
use pipeline::{RunRequest, RunResult};

#[tauri::command]
fn get_assist_status(mode: String, assist: Option<AssistConfig>) -> AssistStatus {
    let cfg = assist.unwrap_or_default();
    let mode = if mode.eq_ignore_ascii_case("assist") {
        "assist"
    } else {
        "standard"
    };
    StubLlmProvider::from_config(cfg).status(mode)
}

#[tauri::command]
fn run_pipeline(request: RunRequest) -> RunResult {
    pipeline::run_pipeline(request)
}

#[tauri::command]
fn app_info() -> serde_json::Value {
    serde_json::json!({
        "name": "Compliancy Checker",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["standard", "assist"],
        "formats": ["docx"],
        "spike": "docx template style merge"
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
            run_pipeline
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
