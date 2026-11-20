use std::path::PathBuf;

use compliancy_core::{
    AssistConfig, AssistRuntime, AssistStatus, IsoFirstRequest, IsoFirstResult, RunRequest,
    RunResult, TemplatePreview,
};
use tauri::{Manager, PhysicalPosition, PhysicalSize, Position, Runtime, Size, WebviewWindow};

/// Default logical width when the work area is wide enough (matches `tauri.conf.json`).
const DEFAULT_WINDOW_WIDTH_LOGICAL: f64 = 1120.0;

/// Size and place the main window inside the monitor work area (excludes the taskbar).
///
/// Height fills the work area; width stays ~1120 logical px unless the work area is narrower.
/// Outer chrome (title bar / borders) is subtracted so the framed window does not cover the taskbar.
fn apply_work_area_default_size<R: Runtime>(window: &WebviewWindow<R>) {
    let Some(monitor) = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
    else {
        return;
    };

    let work = *monitor.work_area();
    if work.size.width == 0 || work.size.height == 0 {
        return;
    }

    let scale = monitor.scale_factor().max(0.1);
    let desired_outer_width =
        ((DEFAULT_WINDOW_WIDTH_LOGICAL * scale).round() as u32).clamp(1, work.size.width);

    let (chrome_w, chrome_h) = match (window.outer_size(), window.inner_size()) {
        (Ok(outer), Ok(inner)) => (
            outer.width.saturating_sub(inner.width),
            outer.height.saturating_sub(inner.height),
        ),
        _ => (0, 0),
    };

    let inner_width = desired_outer_width.saturating_sub(chrome_w).max(1);
    let inner_height = work.size.height.saturating_sub(chrome_h).max(1);

    let x = work.position.x + (work.size.width as i32 - desired_outer_width as i32) / 2;
    let y = work.position.y;

    let _ = window.set_size(Size::Physical(PhysicalSize::new(inner_width, inner_height)));
    let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
}

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

/// ISO card **RUN FIRST** — checklist only, before the main pipeline.
#[tauri::command]
fn run_iso_first(request: IsoFirstRequest) -> IsoFirstResult {
    compliancy_core::run_iso_first(request)
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
        "formats": ["docx", "pdf"],
        "isoStandards": ["9001", "14001", "45001"],
        "features": [
            "docx template style merge",
            "PDF master template cues + PDF export",
            "template trait preview + picker",
            "ISO section checklist export",
            "ISO RUN FIRST pre-check",
            "structured context rules",
            "Assist OpenAI-compatible LLM hook"
        ],
        "notes": {
            "pdf": "PDF masters supply style/layout cues (fonts, colours, page size); export regenerates PDF from extracted text. Scanned PDFs / OCR out of scope."
        },
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                // Size before show to avoid a wrong-size flash; re-apply after show so
                // title-bar / border chrome metrics are accurate on Windows.
                apply_work_area_default_size(&window);
                let _ = window.show();
                apply_work_area_default_size(&window);
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            get_assist_status,
            inspect_template,
            run_pipeline,
            run_iso_first
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
