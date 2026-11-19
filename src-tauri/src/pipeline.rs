//! High-level pipeline orchestration for the app shell.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use docx_core::{self, DocxError, IsoStandard};

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

    let iso_standards: Option<Vec<IsoStandard>> = if req.iso.enabled {
        match docx_core::parse_standards(&req.iso.standards) {
            Ok(standards) if standards.is_empty() => {
                log.push(log_line(
                    "warn",
                    "ISO ruleset enabled but no standards selected — skipping checklist.",
                ));
                None
            }
            Ok(standards) => {
                let labels = standards
                    .iter()
                    .map(|s| s.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                log.push(log_line(
                    "info",
                    format!("ISO checklist enabled: {labels}"),
                ));
                Some(standards)
            }
            Err(e) => {
                log.push(log_line("error", e.to_string()));
                return RunResult {
                    ok: false,
                    files_written,
                    log,
                    assist: assist_status,
                };
            }
        }
    } else {
        log.push(log_line("info", "ISO ruleset off."));
        None
    };

    let context_rules = if req.context.enabled {
        if req.context.text.trim().is_empty() {
            log.push(log_line(
                "warn",
                "Context ruleset enabled but empty — skipping.",
            ));
            None
        } else {
            match docx_core::parse_context_rules(&req.context.text) {
                Ok(rules) if rules.is_empty() => {
                    log.push(log_line(
                        "warn",
                        "Context ruleset parsed to zero rules — skipping.",
                    ));
                    None
                }
                Ok(rules) => {
                    log.push(log_line(
                        "info",
                        format!(
                            "Context ruleset: {} structured rule(s) (line/JSON; no LLM).",
                            rules.len()
                        ),
                    ));
                    if mode == "assist" {
                        log.push(log_line(
                            "info",
                            "Assist freeform NLP remains stubbed; Standard structured rules still apply.",
                        ));
                    }
                    Some(rules)
                }
                Err(e) => {
                    log.push(log_line("error", e.to_string()));
                    return RunResult {
                        ok: false,
                        files_written,
                        log,
                        assist: assist_status,
                    };
                }
            }
        }
    } else {
        log.push(log_line("info", "Context ruleset off."));
        None
    };

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
    let mut iso_reports = Vec::new();
    let mut context_report = docx_core::ContextReport::default();

    for input in &inputs {
        let file_name = input
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_else(|| "output.docx".into());
        let dest = output_dir.join(&file_name);

        let result = if let Some(ref tpl) = template_path {
            docx_core::transform_docx(input, tpl, &dest)
        } else {
            fs::copy(input, &dest)
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
                continue;
            }
        }

        if let Some(ref rules) = context_rules {
            match docx_core::apply_context_to_docx(&dest, rules) {
                Ok(report) => {
                    log.push(log_line(
                        "info",
                        format!(
                            "Context {}: {} applied/replaced, {} required misses",
                            dest.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("document"),
                            report.applied_count(),
                            report.missing_count()
                        ),
                    ));
                    if report.missing_count() > 0 {
                        ok = false;
                    }
                    context_report.extend(report);
                }
                Err(e) => {
                    ok = false;
                    log.push(log_line(
                        "error",
                        format!("Context rules failed for {}: {e}", dest.display()),
                    ));
                }
            }
        }

        if let Some(ref standards) = iso_standards {
            // Check the ingested source content (facts before/without style merge).
            match docx_core::evaluate_docx(input, standards) {
                Ok(report) => {
                    log.push(log_line(
                        "info",
                        format!(
                            "ISO checklist {}: {} present, {} missing",
                            input
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("document"),
                            report.present_count(),
                            report.missing_count()
                        ),
                    ));
                    iso_reports.push(report);
                }
                Err(e) => {
                    ok = false;
                    log.push(log_line(
                        "error",
                        format!("ISO checklist failed for {}: {e}", input.display()),
                    ));
                }
            }
        }
    }

    if context_rules.is_some() && !context_report.findings.is_empty() {
        match docx_core::write_context_exports(&context_report, &output_dir) {
            Ok((csv_path, txt_path)) => {
                log.push(log_line(
                    "info",
                    format!(
                        "Context findings exported: {} applied/replaced, {} required misses",
                        context_report.applied_count(),
                        context_report.missing_count()
                    ),
                ));
                log.push(log_line("info", format!("Wrote {csv_path}")));
                log.push(log_line("info", format!("Wrote {txt_path}")));
                files_written.push(csv_path);
                files_written.push(txt_path);
            }
            Err(e) => {
                ok = false;
                log.push(log_line(
                    "error",
                    format!("Failed to write context rule exports: {e}"),
                ));
            }
        }
    }

    if let Some(ref standards) = iso_standards {
        if !iso_reports.is_empty() {
            let merged = docx_core::merge_reports(iso_reports);
            match docx_core::write_exports(&merged, &output_dir) {
                Ok((csv_path, txt_path)) => {
                    log.push(log_line(
                        "info",
                        format!(
                            "ISO findings exported: {} present, {} missing across {} standard(s)",
                            merged.present_count(),
                            merged.missing_count(),
                            standards.len()
                        ),
                    ));
                    log.push(log_line("info", format!("Wrote {csv_path}")));
                    log.push(log_line("info", format!("Wrote {txt_path}")));
                    files_written.push(csv_path);
                    files_written.push(txt_path);
                }
                Err(e) => {
                    ok = false;
                    log.push(log_line(
                        "error",
                        format!("Failed to write ISO checklist exports: {e}"),
                    ));
                }
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
