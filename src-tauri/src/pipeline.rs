//! High-level pipeline orchestration for the app shell.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use docx_core::{self, DocxError};

use crate::assist::{AssistConfig, AssistStatus, LlmProvider, StubLlmProvider};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsoRuleset {
    pub enabled: bool,
    /// One or more of: 45001, 14001, 9001
    pub standards: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRuleset {
    pub enabled: bool,
    pub template_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRuleset {
    pub enabled: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// `standard` | `assist`
    pub mode: String,
    pub input_paths: Vec<String>,
    pub output_dir: String,
    pub iso: IsoRuleset,
    pub template: TemplateRuleset,
    pub context: ContextRuleset,
    pub assist: Option<AssistConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunLogLine {
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub ok: bool,
    pub files_written: Vec<String>,
    pub log: Vec<RunLogLine>,
    pub assist: AssistStatus,
}

fn log_line(level: &str, message: impl Into<String>) -> RunLogLine {
    RunLogLine {
        level: level.into(),
        message: message.into(),
    }
}

fn collect_docx_inputs(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for raw in paths {
        let p = PathBuf::from(raw);
        if !p.exists() {
            return Err(format!("input path not found: {}", p.display()));
        }
        if p.is_file() {
            if is_docx(&p) {
                out.push(p);
            }
        } else if p.is_dir() {
            for entry in fs::read_dir(&p).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                if path.is_file() && is_docx(&path) {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn is_docx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("docx"))
        .unwrap_or(false)
}

/// Run the local pipeline. Assist semantic steps are stubbed.
pub fn run_pipeline(req: RunRequest) -> RunResult {
    let mut log = Vec::new();
    let mut files_written = Vec::new();

    let mode = if req.mode.eq_ignore_ascii_case("assist") {
        "assist"
    } else {
        "standard"
    };

    let assist_cfg = req.assist.clone().unwrap_or_default();
    let provider = StubLlmProvider::from_config(assist_cfg);
    let assist_status = provider.status(mode);
    log.push(log_line("info", format!("Mode: {mode}")));
    log.push(log_line("info", assist_status.message.clone()));

    if mode == "assist" {
        match provider.complete("ISO gap narrative placeholder") {
            Ok(_) => {}
            Err(msg) => log.push(log_line("warn", msg)),
        }
    }

    if req.iso.enabled {
        let standards = if req.iso.standards.is_empty() {
            "none selected".into()
        } else {
            req.iso.standards.join(", ")
        };
        log.push(log_line(
            "info",
            format!("ISO ruleset stub: standards [{standards}] — checklist not implemented yet"),
        ));
    }

    if req.context.enabled && !req.context.text.trim().is_empty() {
        let preview: String = req.context.text.chars().take(120).collect();
        log.push(log_line(
            "info",
            format!("Context ruleset noted (structured apply TODO): {preview}"),
        ));
        if mode == "assist" {
            log.push(log_line(
                "info",
                "Assist would interpret freeform context here (stubbed).",
            ));
        }
    }

    let inputs = match collect_docx_inputs(&req.input_paths) {
        Ok(v) => v,
        Err(e) => {
            log.push(log_line("error", e));
            return RunResult {
                ok: false,
                files_written,
                log,
                assist: assist_status,
            };
        }
    };

    if inputs.is_empty() {
        log.push(log_line(
            "error",
            "No .docx inputs found. Pick files or a folder containing .docx.",
        ));
        return RunResult {
            ok: false,
            files_written,
            log,
            assist: assist_status,
        };
    }

    let output_dir = PathBuf::from(&req.output_dir);
    if let Err(e) = fs::create_dir_all(&output_dir) {
        log.push(log_line("error", format!("Cannot create output dir: {e}")));
        return RunResult {
            ok: false,
            files_written,
            log,
            assist: assist_status,
        };
    }

    let template_path = if req.template.enabled {
        match req.template.template_path.as_ref() {
            Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
            _ => {
                log.push(log_line(
                    "error",
                    "Template ruleset enabled but no master template path set.",
                ));
                return RunResult {
                    ok: false,
                    files_written,
                    log,
                    assist: assist_status,
                };
            }
        }
    } else {
        None
    };

    if let Some(ref tpl) = template_path {
        if !tpl.exists() {
            log.push(log_line(
                "error",
                format!("Master template not found: {}", tpl.display()),
            ));
            return RunResult {
                ok: false,
                files_written,
                log,
                assist: assist_status,
            };
        }
        log.push(log_line(
            "info",
            format!("Using master template: {}", tpl.display()),
        ));
    } else {
        log.push(log_line(
            "warn",
            "Template ruleset off — copying inputs without style merge.",
        ));
    }

    let mut ok = true;
    for input in inputs {
        let file_name = input
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_else(|| "output.docx".into());
        let dest = output_dir.join(file_name);

        let result = if let Some(ref tpl) = template_path {
            docx_core::transform_docx(&input, tpl, &dest)
        } else {
            fs::copy(&input, &dest)
                .map(|_| ())
                .map_err(DocxError::from)
        };

        match result {
            Ok(()) => {
                log.push(log_line(
                    "info",
                    format!("Wrote {}", dest.display()),
                ));
                files_written.push(dest.display().to_string());
            }
            Err(e) => {
                ok = false;
                log.push(log_line(
                    "error",
                    format!("Failed {}: {e}", input.display()),
                ));
            }
        }
    }

    RunResult {
        ok,
        files_written,
        log,
        assist: assist_status,
    }
}
