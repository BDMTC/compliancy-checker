//! High-level pipeline orchestration for the app shell.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use docx_core::{self, DocText, DocxError, IsoStandard};

use crate::assist::{
    self, interpret_freeform_context, iso_gap_narrative, looks_like_freeform_context, AssistConfig,
    AssistRuntime, AssistStatus,
};
use crate::ingest::{
    collect_document_inputs, convert_doc_to_docx, count_files_in_dir, resolve_output_ext,
    CollectedInput, InputKind, RootScan,
};

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
    /// Selected trait ids from the template preview picker.
    /// `None` = apply all detected traits; `Some([])` = apply none; otherwise the subset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_trait_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRuleset {
    pub enabled: bool,
    pub text: String,
}

fn default_output_format() -> String {
    "match".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// `standard` | `assist`
    pub mode: String,
    pub input_paths: Vec<String>,
    pub output_dir: String,
    /// `match` (default: per-file) | `docx` | `pdf` (convert-all).
    #[serde(default = "default_output_format")]
    pub output_format: String,
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

/// Request for ISO card **RUN FIRST** — checklist only, before the main pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsoFirstRequest {
    pub input_paths: Vec<String>,
    /// One or more of: 45001, 14001, 9001
    pub standards: Vec<String>,
    /// Optional: when set, write `iso-checklist.csv` + `iso-checklist-report.txt` here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsoFirstResult {
    /// Command completed without hard errors (I/O, bad standards, etc.).
    pub ok: bool,
    /// All checklist clauses present (`missing_count == 0`).
    pub passed: bool,
    /// Human labels for the selected standards, e.g. `["ISO 45001", "ISO 9001"]`.
    pub standards_labels: Vec<String>,
    /// Pass toast copy: `Compliancy passes: {selected}`.
    pub pass_message: String,
    /// Missing clause reasons (from checklist findings) for the fail popup.
    pub reasons: Vec<String>,
    pub csv: String,
    pub report_text: String,
    /// Combined CSV + text report for the Print action.
    pub combined_report: String,
    pub files_written: Vec<String>,
    pub log: Vec<RunLogLine>,
}

fn combined_iso_print_report(csv: &str, report_text: &str) -> String {
    let mut out = String::new();
    out.push_str("Compliancy Checker — ISO RUN FIRST combined report\n");
    out.push_str("==================================================\n\n");
    out.push_str("--- Text report ---\n");
    out.push_str(report_text);
    if !report_text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n--- CSV ---\n");
    out.push_str(csv);
    if !csv.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn missing_reasons(report: &docx_core::ChecklistReport) -> Vec<String> {
    report
        .findings
        .iter()
        .filter(|f| f.status == docx_core::Presence::Missing)
        .map(|f| {
            format!(
                "{} — {} {} ({})",
                f.document,
                f.standard.label(),
                f.clause,
                f.title
            )
        })
        .collect()
}

/// Run the ISO section checklist only (no template / context / document export).
/// Used by the ISO card **RUN FIRST** control.
pub fn run_iso_first(req: IsoFirstRequest) -> IsoFirstResult {
    let mut log = Vec::new();
    let mut files_written = Vec::new();

    log.push(log_line("info", "ISO RUN FIRST — checklist before pipeline."));

    if req.input_paths.is_empty() {
        log.push(log_line("error", "Choose input files or a folder."));
        return IsoFirstResult {
            ok: false,
            passed: false,
            standards_labels: Vec::new(),
            pass_message: String::new(),
            reasons: vec!["Choose input files or a folder.".into()],
            csv: String::new(),
            report_text: String::new(),
            combined_report: String::new(),
            files_written,
            log,
        };
    }

    let standards = match docx_core::parse_standards(&req.standards) {
        Ok(s) if s.is_empty() => {
            log.push(log_line(
                "error",
                "Select at least one ISO standard (9001 / 14001 / 45001).",
            ));
            return IsoFirstResult {
                ok: false,
                passed: false,
                standards_labels: Vec::new(),
                pass_message: String::new(),
                reasons: vec![
                    "Select at least one ISO standard (9001 / 14001 / 45001).".into(),
                ],
                csv: String::new(),
                report_text: String::new(),
                combined_report: String::new(),
                files_written,
                log,
            };
        }
        Ok(s) => s,
        Err(e) => {
            log.push(log_line("error", e.to_string()));
            return IsoFirstResult {
                ok: false,
                passed: false,
                standards_labels: Vec::new(),
                pass_message: String::new(),
                reasons: vec![e.to_string()],
                csv: String::new(),
                report_text: String::new(),
                combined_report: String::new(),
                files_written,
                log,
            };
        }
    };

    let standards_labels: Vec<String> = standards.iter().map(|s| s.label().to_string()).collect();
    let selected = standards_labels.join(", ");
    let pass_message = format!("Compliancy passes: {selected}");
    log.push(log_line(
        "info",
        format!("ISO RUN FIRST standards: {selected}"),
    ));

    let collected = match collect_document_inputs(&req.input_paths) {
        Ok(c) if c.inputs.is_empty() => {
            log.push(log_line(
                "error",
                "No .docx / .pdf / .doc inputs found for ISO RUN FIRST.",
            ));
            return IsoFirstResult {
                ok: false,
                passed: false,
                standards_labels,
                pass_message,
                reasons: vec![
                    "No .docx / .pdf / .doc inputs found for ISO RUN FIRST.".into(),
                ],
                csv: String::new(),
                report_text: String::new(),
                combined_report: String::new(),
                files_written,
                log,
            };
        }
        Ok(c) => c,
        Err(e) => {
            log.push(log_line("error", e.clone()));
            return IsoFirstResult {
                ok: false,
                passed: false,
                standards_labels,
                pass_message,
                reasons: vec![e],
                csv: String::new(),
                report_text: String::new(),
                combined_report: String::new(),
                files_written,
                log,
            };
        }
    };

    push_scan_logs(&mut log, &collected.roots);

    let work_dir = req
        .output_dir
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".compliancy-iso-work");
    let _ = fs::create_dir_all(&work_dir);

    log.push(log_line(
        "info",
        format!("Evaluating {} document(s)…", collected.inputs.len()),
    ));
    for limit in pdf_core::pdf_text_limits().iter().take(2) {
        if collected.inputs.iter().any(|i| i.kind == InputKind::Pdf) {
            log.push(log_line("info", format!("PDF text limit: {limit}")));
        }
    }

    let mut reports = Vec::new();
    let mut ok = true;
    for input in &collected.inputs {
        match evaluate_input_iso(input, &standards, &work_dir, &mut log) {
            Ok(report) => {
                log.push(log_line(
                    "info",
                    format!(
                        "ISO checklist {}: {} present, {} missing",
                        input
                            .path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("document"),
                        report.present_count(),
                        report.missing_count()
                    ),
                ));
                reports.push(report);
            }
            Err(e) => {
                ok = false;
                log.push(log_line(
                    "error",
                    format!("ISO checklist failed for {}: {e}", input.path.display()),
                ));
            }
        }
    }
    let _ = fs::remove_dir_all(&work_dir);

    if reports.is_empty() {
        return IsoFirstResult {
            ok: false,
            passed: false,
            standards_labels,
            pass_message,
            reasons: vec!["ISO checklist produced no findings (evaluation failed).".into()],
            csv: String::new(),
            report_text: String::new(),
            combined_report: String::new(),
            files_written,
            log,
        };
    }

    let merged = docx_core::merge_reports(reports);
    let csv = merged.to_csv();
    let report_text = merged.to_text_report();
    let combined_report = combined_iso_print_report(&csv, &report_text);
    let reasons = missing_reasons(&merged);
    let passed = ok && reasons.is_empty();

    if let Some(ref out) = req.output_dir {
        let out = out.trim();
        if !out.is_empty() {
            let output_dir = PathBuf::from(out);
            match docx_core::write_exports(&merged, &output_dir) {
                Ok((csv_path, txt_path)) => {
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

    if passed {
        log.push(log_line("info", pass_message.clone()));
    } else {
        log.push(log_line(
            "warn",
            format!(
                "ISO RUN FIRST failed: {} missing clause(s).",
                reasons.len()
            ),
        ));
    }

    IsoFirstResult {
        ok,
        passed,
        standards_labels,
        pass_message,
        reasons,
        csv,
        report_text,
        combined_report,
        files_written,
        log,
    }
}

fn log_line(level: &str, message: impl Into<String>) -> RunLogLine {
    RunLogLine {
        level: level.into(),
        message: message.into(),
    }
}

fn push_scan_logs(log: &mut Vec<RunLogLine>, roots: &[RootScan]) {
    for root in roots {
        if root.is_directory {
            log.push(log_line(
                "info",
                format!(
                    "Scan {}: {} file(s) in directory — .docx={}, .pdf={}, .doc={}, skipped unsupported={}",
                    root.path.display(),
                    root.total_files_in_dir,
                    root.docx,
                    root.pdf,
                    root.doc,
                    root.skipped_names.len()
                ),
            ));
            for name in &root.skipped_names {
                log.push(log_line(
                    "info",
                    format!("Skipped unsupported file in {}: {name}", root.path.display()),
                ));
            }
        } else {
            log.push(log_line(
                "info",
                format!(
                    "Scan (direct picks under {}): .docx={}, .pdf={}, .doc={}",
                    root.path.display(),
                    root.docx,
                    root.pdf,
                    root.doc
                ),
            ));
        }
    }
}

/// Per-directory counters for the required run-summary block.
#[derive(Debug, Default, Clone)]
struct DirCounters {
    successful: usize,
    unsuccessful: usize,
    skipped: usize,
}

fn push_directory_summary(
    log: &mut Vec<RunLogLine>,
    label: &str,
    dir_path: &Path,
    total_in_dir: usize,
    counters: &DirCounters,
    output_dir: &Path,
) {
    let complete = counters.successful;
    let fail = counters.unsuccessful;
    let out_count = count_files_in_dir(output_dir);
    log.push(log_line("info", "——— Run summary ———"));
    log.push(log_line(
        "info",
        format!("Directory information: {label} {}", dir_path.display()),
    ));
    log.push(log_line(
        "info",
        format!("Total file count in the directory: {total_in_dir}"),
    ));
    log.push(log_line(
        "info",
        format!("Successful output: {}", counters.successful),
    ));
    log.push(log_line(
        "info",
        format!("Unsuccessful output: {}", counters.unsuccessful),
    ));
    log.push(log_line(
        "info",
        format!("Skipped output: {}", counters.skipped),
    ));
    log.push(log_line("info", format!("Total complete: {complete}")));
    log.push(log_line("info", format!("Total fail: {fail}")));
    log.push(log_line(
        "info",
        format!("Total file count in the output directory: {out_count}"),
    ));
}

fn evaluate_input_iso(
    input: &CollectedInput,
    standards: &[IsoStandard],
    work_dir: &Path,
    log: &mut Vec<RunLogLine>,
) -> Result<docx_core::ChecklistReport, String> {
    match input.kind {
        InputKind::Docx => docx_core::evaluate_docx(&input.path, standards).map_err(|e| e.to_string()),
        InputKind::Pdf => {
            let text = pdf_core::extract_pdf_text(&input.path).map_err(|e| e.to_string())?;
            if text.plain.trim().is_empty() {
                log.push(log_line(
                    "warn",
                    format!(
                        "PDF text extract empty for {} (scanned/image-only or undecodable); ISO may under-report.",
                        input.path.display()
                    ),
                ));
            }
            let label = input
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("document.pdf");
            Ok(docx_core::evaluate_text(label, &text, standards))
        }
        InputKind::Doc => {
            let converted = convert_doc_to_docx(&input.path, work_dir)?;
            log.push(log_line(
                "info",
                format!(
                    "Converted legacy .doc → {}",
                    converted
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("converted.docx")
                ),
            ));
            docx_core::evaluate_docx(&converted, standards).map_err(|e| e.to_string())
        }
    }
}

/// Materialise a working `.docx` for template/context (Word path or PDF→Word).
fn materialise_working_docx(
    input: &CollectedInput,
    work_dir: &Path,
    log: &mut Vec<RunLogLine>,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    match input.kind {
        InputKind::Docx => Ok((input.path.clone(), None)),
        InputKind::Doc => {
            let converted = convert_doc_to_docx(&input.path, work_dir)?;
            log.push(log_line(
                "info",
                format!(
                    "Converted legacy .doc for ingest: {} → {}",
                    input.path.display(),
                    converted.display()
                ),
            ));
            Ok((converted.clone(), Some(converted)))
        }
        InputKind::Pdf => {
            let text = pdf_core::extract_pdf_text(&input.path).map_err(|e| e.to_string())?;
            let stem = input
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("from-pdf");
            let dest = work_dir.join(format!("{stem}-from-pdf.docx"));
            let lines: Vec<String> = text.plain.lines().map(|s| s.to_string()).collect();
            docx_core::write_docx_from_plain_lines(&dest, &lines).map_err(|e| e.to_string())?;
            log.push(log_line(
                "info",
                format!(
                    "Materialised PDF text → {} ({} line(s))",
                    dest.display(),
                    lines.len()
                ),
            ));
            Ok((dest.clone(), Some(dest)))
        }
    }
}

fn source_text_for_iso(
    input: &CollectedInput,
    working_docx: &Path,
) -> Result<DocText, String> {
    match input.kind {
        InputKind::Pdf => pdf_core::extract_pdf_text(&input.path).map_err(|e| e.to_string()),
        InputKind::Docx | InputKind::Doc => {
            docx_core::extract_docx_text(working_docx).map_err(|e| e.to_string())
        }
    }
}

/// Run the local pipeline. Assist semantic steps only run in Assist mode when configured.
pub fn run_pipeline(req: RunRequest) -> RunResult {
    let mut log = Vec::new();
    let mut files_written = Vec::new();

    let mode = if req.mode.eq_ignore_ascii_case("assist") {
        "assist"
    } else {
        "standard"
    };

    let assist_rt = AssistRuntime::for_mode(mode, req.assist.clone());
    let assist_status = assist_rt.status.clone();
    log.push(log_line("info", format!("Mode: {mode}")));
    log.push(log_line("info", assist_status.message.clone()));

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

    let mut freeform_context: Option<String> = None;
    let mut structured_context_summary: Option<String> = None;

    let context_rules = if req.context.enabled {
        if req.context.text.trim().is_empty() {
            log.push(log_line(
                "warn",
                "Context ruleset enabled but empty — skipping.",
            ));
            None
        } else {
            let raw = req.context.text.clone();
            let wants_freeform = mode == "assist" && looks_like_freeform_context(&raw);
            match docx_core::parse_context_rules(&raw) {
                Ok(rules) if rules.is_empty() => {
                    log.push(log_line(
                        "warn",
                        "Context ruleset parsed to zero rules — skipping structured apply.",
                    ));
                    if wants_freeform || mode == "assist" {
                        freeform_context = Some(raw);
                    }
                    None
                }
                Ok(rules) => {
                    log.push(log_line(
                        "info",
                        format!(
                            "Context ruleset: {} structured rule(s) (line/JSON; applied locally).",
                            rules.len()
                        ),
                    ));
                    structured_context_summary = Some(format!("{} structured rule(s) parsed", rules.len()));
                    if wants_freeform {
                        log.push(log_line(
                            "info",
                            "Assist will also interpret freeform lines that are outside the rule language.",
                        ));
                        freeform_context = Some(raw);
                    }
                    Some(rules)
                }
                Err(e) => {
                    if mode == "assist" {
                        log.push(log_line(
                            "warn",
                            format!(
                                "Structured context parse failed ({e}); Assist will try freeform interpretation."
                            ),
                        ));
                        freeform_context = Some(raw);
                        None
                    } else {
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
        }
    } else {
        log.push(log_line("info", "Context ruleset off."));
        None
    };

    let collected = match collect_document_inputs(&req.input_paths) {
        Ok(c) => c,
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

    push_scan_logs(&mut log, &collected.roots);

    if collected.inputs.is_empty() {
        log.push(log_line(
            "error",
            "No .docx / .pdf / .doc inputs found. Pick files or a folder containing those types.",
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

    let work_dir = output_dir.join(".compliancy-work");
    let _ = fs::create_dir_all(&work_dir);

    let fmt_raw = req.output_format.trim().to_ascii_lowercase();
    let output_mode = if fmt_raw == "pdf" {
        "pdf (convert-all)"
    } else if fmt_raw == "docx" || fmt_raw == "word" {
        "docx (convert-all)"
    } else {
        "match input (per-file: .doc/.docx→.docx, .pdf→.pdf)"
    };
    log.push(log_line(
        "info",
        format!("Output format: {output_mode}"),
    ));
    if collected.inputs.iter().any(|i| i.kind == InputKind::Pdf) {
        for limit in pdf_core::pdf_text_limits().iter().take(2) {
            log.push(log_line("info", format!("PDF text limit: {limit}")));
        }
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

    let mut transform_opts = docx_core::TransformOptions::all();
    let mut template_preview: Option<docx_core::TemplatePreview> = None;
    // Effective `.docx` master used by Open XML merge (original or PDF-synthesised).
    let mut merge_template: Option<PathBuf> = None;
    let mut synthetic_template_path: Option<PathBuf> = None;

    let selection = docx_core::TraitSelection {
        selected_ids: req.template.selected_trait_ids.clone(),
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

        if pdf_core::is_pdf(tpl) {
            let synth = output_dir.join(".compliancy-synthetic-master.docx");
            match pdf_core::materialise_docx_master(tpl, &selection, &synth) {
                Ok(Some((preview, opts))) => {
                    let selected = match &req.template.selected_trait_ids {
                        None => preview.traits.len(),
                        Some(ids) => ids.len(),
                    };
                    log.push(log_line(
                        "info",
                        format!(
                            "Using PDF master template: {} ({} / {} trait(s); fonts={}, colours={}, background={})",
                            tpl.display(),
                            selected,
                            preview.traits.len(),
                            opts.apply_fonts,
                            opts.apply_colours,
                            opts.apply_background
                        ),
                    ));
                    for limit in preview.limits.iter().take(3) {
                        log.push(log_line("info", format!("PDF limit: {limit}")));
                    }
                    transform_opts = opts;
                    template_preview = Some(preview);
                    merge_template = Some(synth.clone());
                    synthetic_template_path = Some(synth);
                }
                Ok(None) => {
                    log.push(log_line(
                        "error",
                        "PDF master materialisation returned no template.",
                    ));
                    return RunResult {
                        ok: false,
                        files_written,
                        log,
                        assist: assist_status,
                    };
                }
                Err(e) => {
                    log.push(log_line(
                        "error",
                        format!("Failed to inspect/materialise PDF master: {e}"),
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
            match docx_core::options_from_template(tpl, &selection) {
                Ok((preview, opts)) => {
                    let selected = match &req.template.selected_trait_ids {
                        None => preview.traits.len(),
                        Some(ids) => ids.len(),
                    };
                    log.push(log_line(
                        "info",
                        format!(
                            "Using master template: {} ({} / {} trait(s); fonts={}, colours={}, background={})",
                            tpl.display(),
                            selected,
                            preview.traits.len(),
                            opts.apply_fonts,
                            opts.apply_colours,
                            opts.apply_background
                        ),
                    ));
                    transform_opts = opts;
                    template_preview = Some(preview);
                    merge_template = Some(tpl.clone());
                }
                Err(e) => {
                    log.push(log_line(
                        "error",
                        format!("Failed to inspect master template: {e}"),
                    ));
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
        log.push(log_line(
            "warn",
            "Template ruleset off — copying inputs without style merge.",
        ));
    }

    let mut ok = true;
    let mut iso_reports = Vec::new();
    let mut context_report = docx_core::ContextReport::default();

    // Counters keyed by source_root path string.
    let mut dir_counters: BTreeMap<String, DirCounters> = BTreeMap::new();
    let mut dir_totals: BTreeMap<String, usize> = BTreeMap::new();
    for root in &collected.roots {
        let key = root.path.display().to_string();
        dir_counters.entry(key.clone()).or_default().skipped = root.skipped_names.len();
        dir_totals.insert(
            key,
            if root.is_directory {
                root.total_files_in_dir
            } else {
                root.supported_count()
            },
        );
    }
    let mut overall = DirCounters::default();
    for root in &collected.roots {
        overall.skipped += root.skipped_names.len();
    }

    for input in &collected.inputs {
        let root_key = input.source_root.display().to_string();
        let counters = dir_counters.entry(root_key).or_default();
        let stem = input
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let out_ext = resolve_output_ext(&req.output_format, input.kind);

        let working = match materialise_working_docx(input, &work_dir, &mut log) {
            Ok(w) => w,
            Err(e) => {
                ok = false;
                counters.unsuccessful += 1;
                overall.unsuccessful += 1;
                log.push(log_line(
                    "error",
                    format!("Unsuccessful {}: {e}", input.path.display()),
                ));
                continue;
            }
        };
        let (working_docx, _temp_docx) = working;

        // Word path: Open XML merge. PDF path with PDF output: cues on export (below).
        let use_docx_merge = out_ext == "docx"
            || input.kind == InputKind::Docx
            || input.kind == InputKind::Doc
            || context_rules.is_some();

        let docx_dest = if out_ext == "docx" {
            output_dir.join(format!("{stem}.docx"))
        } else {
            work_dir.join(format!("{stem}-intermediate.docx"))
        };

        let mut file_ok = true;

        if use_docx_merge {
            // PDF→PDF uses cues on export (not Open XML merge). Word keeps docx merge.
            let skip_merge = input.kind == InputKind::Pdf && out_ext == "pdf";
            let result = if let Some(ref tpl) = merge_template {
                if skip_merge {
                    fs::copy(&working_docx, &docx_dest)
                        .map(|_| ())
                        .map_err(DocxError::from)
                } else {
                    docx_core::transform_docx_with_options(
                        &working_docx,
                        tpl,
                        &docx_dest,
                        transform_opts,
                    )
                }
            } else {
                fs::copy(&working_docx, &docx_dest)
                    .map(|_| ())
                    .map_err(DocxError::from)
            };

            match result {
                Ok(()) => {
                    if out_ext == "docx" {
                        log.push(log_line(
                            "info",
                            format!("Wrote {}", docx_dest.display()),
                        ));
                        files_written.push(docx_dest.display().to_string());
                    } else {
                        log.push(log_line(
                            "info",
                            format!("Prepared intermediate {}", docx_dest.display()),
                        ));
                    }
                }
                Err(e) => {
                    file_ok = false;
                    ok = false;
                    log.push(log_line(
                        "error",
                        format!("Failed {}: {e}", input.path.display()),
                    ));
                }
            }
        } else if out_ext == "pdf" {
            // PDF→PDF without context: materialise text blocks directly later.
            let _ = fs::copy(&working_docx, &docx_dest);
        }

        if file_ok {
            if let Some(ref rules) = context_rules {
                match docx_core::apply_context_to_docx(&docx_dest, rules) {
                    Ok(report) => {
                        log.push(log_line(
                            "info",
                            format!(
                                "Context {}: {} applied/replaced, {} required misses",
                                docx_dest
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("document"),
                                report.applied_count(),
                                report.missing_count()
                            ),
                        ));
                        if report.missing_count() > 0 {
                            ok = false;
                            file_ok = false;
                        }
                        context_report.extend(report);
                    }
                    Err(e) => {
                        file_ok = false;
                        ok = false;
                        log.push(log_line(
                            "error",
                            format!("Context rules failed for {}: {e}", docx_dest.display()),
                        ));
                    }
                }
            }
        }

        if file_ok && out_ext == "pdf" {
            let pdf_dest = output_dir.join(format!("{stem}.pdf"));
            let cues = match &template_preview {
                Some(preview) => pdf_core::cues_from_preview(preview, &transform_opts),
                None => pdf_core::PdfStyleCues::default(),
            };
            let export_opts = pdf_core::PdfExportOptions {
                title: stem.to_string(),
                cues,
                apply_fonts: transform_opts.apply_fonts,
                apply_colours: transform_opts.apply_colours,
                apply_background: transform_opts.apply_background,
            };
            // PDF path uses style cues; Word path already merged above when applicable.
            match pdf_core::write_pdf_from_docx(&docx_dest, &pdf_dest, &export_opts) {
                Ok(()) => {
                    log.push(log_line("info", format!("Wrote {}", pdf_dest.display())));
                    files_written.push(pdf_dest.display().to_string());
                    if docx_dest.starts_with(&work_dir) || out_ext == "pdf" {
                        let _ = fs::remove_file(&docx_dest);
                    }
                }
                Err(e) => {
                    file_ok = false;
                    ok = false;
                    log.push(log_line(
                        "error",
                        format!("PDF export failed for {}: {e}", input.path.display()),
                    ));
                }
            }
        }

        if let Some(ref standards) = iso_standards {
            match source_text_for_iso(input, &working_docx) {
                Ok(text) => {
                    let label = input
                        .path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("document");
                    let report = docx_core::evaluate_text(label, &text, standards);
                    log.push(log_line(
                        "info",
                        format!(
                            "ISO checklist {}: {} present, {} missing",
                            label,
                            report.present_count(),
                            report.missing_count()
                        ),
                    ));
                    iso_reports.push(report);
                }
                Err(e) => {
                    ok = false;
                    file_ok = false;
                    log.push(log_line(
                        "error",
                        format!("ISO checklist failed for {}: {e}", input.path.display()),
                    ));
                }
            }
        }

        if file_ok {
            counters.successful += 1;
            overall.successful += 1;
        } else {
            counters.unsuccessful += 1;
            overall.unsuccessful += 1;
        }
    }

    if let Some(synth) = synthetic_template_path {
        let _ = fs::remove_file(synth);
    }
    let _ = fs::remove_dir_all(&work_dir);

    // Per active input directory (+ overall when multi-path).
    let multi = collected.roots.len() > 1
        || req.input_paths.len() > 1
        || collected.roots.iter().filter(|r| r.is_directory).count() > 1;
    for root in &collected.roots {
        let key = root.path.display().to_string();
        let counters = dir_counters.get(&key).cloned().unwrap_or_default();
        let total = *dir_totals.get(&key).unwrap_or(&0);
        let label = if root.is_directory {
            "active input directory"
        } else {
            "direct picks under"
        };
        push_directory_summary(&mut log, label, &root.path, total, &counters, &output_dir);
    }
    if multi {
        let total_all: usize = dir_totals.values().sum();
        push_directory_summary(
            &mut log,
            "OVERALL across",
            Path::new("(all input paths)"),
            total_all,
            &overall,
            &output_dir,
        );
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

    let mut iso_text_for_assist: Option<String> = None;

    if let Some(ref standards) = iso_standards {
        if !iso_reports.is_empty() {
            let merged = docx_core::merge_reports(iso_reports);
            iso_text_for_assist = Some(merged.to_text_report());
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

    // Assist semantic steps — only when Assist mode has a configured (live or mock) provider.
    if mode == "assist" && assist_rt.can_call() {
        if let Some(ref checklist_txt) = iso_text_for_assist {
            match iso_gap_narrative(assist_rt.provider.as_ref(), checklist_txt) {
                Ok(narrative) => {
                    let path = output_dir.join("iso-gap-narrative.txt");
                    match assist::write_text_export(&path, &narrative) {
                        Ok(()) => {
                            let path_s = path.display().to_string();
                            log.push(log_line(
                                "info",
                                format!(
                                    "Assist ISO gap narrative via '{}' ({} chars).",
                                    assist_rt.provider.name(),
                                    narrative.len()
                                ),
                            ));
                            log.push(log_line("info", format!("Wrote {path_s}")));
                            files_written.push(path_s);
                        }
                        Err(e) => {
                            ok = false;
                            log.push(log_line(
                                "error",
                                format!("Failed to write ISO gap narrative: {e}"),
                            ));
                        }
                    }
                }
                Err(e) => {
                    log.push(log_line(
                        "warn",
                        format!("Assist ISO gap narrative skipped: {e}"),
                    ));
                }
            }
        }

        if let Some(ref freeform) = freeform_context {
            match interpret_freeform_context(
                assist_rt.provider.as_ref(),
                freeform,
                structured_context_summary.as_deref(),
            ) {
                Ok(interpretation) => {
                    let path = output_dir.join("assist-context-interpretation.txt");
                    match assist::write_text_export(&path, &interpretation) {
                        Ok(()) => {
                            let path_s = path.display().to_string();
                            log.push(log_line(
                                "info",
                                format!(
                                    "Assist freeform context interpretation via '{}' ({} chars).",
                                    assist_rt.provider.name(),
                                    interpretation.len()
                                ),
                            ));
                            log.push(log_line("info", format!("Wrote {path_s}")));
                            files_written.push(path_s);
                        }
                        Err(e) => {
                            ok = false;
                            log.push(log_line(
                                "error",
                                format!("Failed to write Assist context interpretation: {e}"),
                            ));
                        }
                    }
                }
                Err(e) => {
                    log.push(log_line(
                        "warn",
                        format!("Assist freeform context skipped: {e}"),
                    ));
                }
            }
        }
    } else if mode == "assist" && !assist_rt.can_call() {
        if iso_text_for_assist.is_some() || freeform_context.is_some() {
            log.push(log_line(
                "info",
                "Assist semantic steps skipped (provider not configured). Checklist/context still ran locally.",
            ));
        }
    }

    RunResult {
        ok,
        files_written,
        log,
        assist: assist_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_core::{write_minimal_docx, SAMPLE_STYLES};
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn fixture_docx(dir: &Path) -> PathBuf {
        let path = dir.join("sample.docx");
        write_minimal_docx(
            &path,
            "Heading1",
            "Normal",
            &[
                ("title", "4.1 Context of the organization"),
                ("body", "OH&S policy and worker consultation."),
            ],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();
        path
    }

    #[test]
    fn standard_mode_makes_no_assist_exports() {
        let _g = env_lock().lock().unwrap();
        std::env::remove_var(assist::ENV_MOCK);
        std::env::remove_var(assist::ENV_API_KEY);
        std::env::remove_var(assist::ENV_PROVIDER);

        let dir = std::env::temp_dir().join("compliancy-assist-pipeline-std");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let input = fixture_docx(&dir);
        let out = dir.join("out");

        let result = run_pipeline(RunRequest {
            mode: "standard".into(),
            input_paths: vec![input.display().to_string()],
            output_dir: out.display().to_string(),
            output_format: "docx".into(),
            iso: IsoRuleset {
                enabled: true,
                standards: vec!["45001".into()],
            },
            template: TemplateRuleset {
                enabled: false,
                template_path: None,
                selected_trait_ids: None,
            },
            context: ContextRuleset {
                enabled: true,
                text: "force_heading: Scope\nrequire_section: Context of the organization".into(),
            },
            assist: Some(AssistConfig {
                provider: "openai".into(),
                api_key: Some("sk-should-not-be-used".into()),
                endpoint: None,
                model: None,
            }),
        });

        assert!(result.ok, "{:?}", result.log);
        assert!(!result.assist.live_calls_enabled);
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Standard mode") && l.message.contains("offline")));
        assert!(!result
            .files_written
            .iter()
            .any(|p| p.ends_with("iso-gap-narrative.txt")));
        assert!(!result
            .files_written
            .iter()
            .any(|p| p.ends_with("assist-context-interpretation.txt")));
    }

    #[test]
    fn assist_mock_writes_iso_and_context_fixtures() {
        let _g = env_lock().lock().unwrap();
        std::env::set_var(assist::ENV_MOCK, "1");
        std::env::remove_var(assist::ENV_API_KEY);

        let dir = std::env::temp_dir().join("compliancy-assist-pipeline-mock");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let input = fixture_docx(&dir);
        let out = dir.join("out");

        let result = run_pipeline(RunRequest {
            mode: "assist".into(),
            input_paths: vec![input.display().to_string()],
            output_dir: out.display().to_string(),
            output_format: "docx".into(),
            iso: IsoRuleset {
                enabled: true,
                standards: vec!["45001".into()],
            },
            template: TemplateRuleset {
                enabled: false,
                template_path: None,
                selected_trait_ids: None,
            },
            context: ContextRuleset {
                enabled: true,
                text: "Please make the tone more formal and ensure residual OH&S risk is covered."
                    .into(),
            },
            assist: Some(AssistConfig {
                provider: "mock".into(),
                api_key: None,
                endpoint: None,
                model: None,
            }),
        });

        std::env::remove_var(assist::ENV_MOCK);

        assert!(result.ok, "{:?}", result.log);
        assert_eq!(result.assist.provider, "mock");
        assert!(result
            .files_written
            .iter()
            .any(|p| p.ends_with("iso-gap-narrative.txt")));
        assert!(result
            .files_written
            .iter()
            .any(|p| p.ends_with("assist-context-interpretation.txt")));

        let narrative = fs::read_to_string(out.join("iso-gap-narrative.txt")).unwrap();
        assert!(narrative.contains("MOCK ISO gap narrative"));
        let interpretation =
            fs::read_to_string(out.join("assist-context-interpretation.txt")).unwrap();
        assert!(interpretation.contains("MOCK freeform context"));
    }

    #[test]
    fn pdf_output_and_pdf_master_template() {
        let dir = std::env::temp_dir().join("compliancy-pdf-pipeline");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let input = fixture_docx(&dir);
        let out = dir.join("out");

        let master = dir.join("master.pdf");
        pdf_core::write_pdf_from_blocks(
            &master,
            &[
                ("heading", "Brand Heading"),
                ("body", "Brand body sample."),
            ],
            &pdf_core::PdfExportOptions {
                title: "Master".into(),
                cues: pdf_core::PdfStyleCues {
                    body_font: "Helvetica".into(),
                    heading_font: "Helvetica-Bold".into(),
                    body_colour: "1F4E79".into(),
                    heading_colour: "C45911".into(),
                    page_width_pt: 612.0,
                    page_height_pt: 792.0,
                    background_fill: Some("F5F7FA".into()),
                },
                apply_fonts: true,
                apply_colours: true,
                apply_background: true,
            },
        )
        .unwrap();

        let result = run_pipeline(RunRequest {
            mode: "standard".into(),
            input_paths: vec![input.display().to_string()],
            output_dir: out.display().to_string(),
            output_format: "pdf".into(),
            iso: IsoRuleset {
                enabled: false,
                standards: vec![],
            },
            template: TemplateRuleset {
                enabled: true,
                template_path: Some(master.display().to_string()),
                selected_trait_ids: None,
            },
            context: ContextRuleset {
                enabled: false,
                text: String::new(),
            },
            assist: None,
        });

        assert!(result.ok, "{:?}", result.log);
        assert!(result
            .files_written
            .iter()
            .any(|p| p.ends_with(".pdf")));
        assert!(!result
            .files_written
            .iter()
            .any(|p| p.ends_with("sample.docx")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("PDF master template")));
        let pdf_path = out.join("sample.pdf");
        assert!(pdf_path.exists());
        assert!(pdf_path.metadata().unwrap().len() > 200);
    }

    #[test]
    fn iso_run_first_fails_with_reasons_on_partial_doc() {
        let dir = std::env::temp_dir().join("compliancy-iso-run-first-fail");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let input = fixture_docx(&dir);
        let out = dir.join("out");

        let result = run_iso_first(IsoFirstRequest {
            input_paths: vec![input.display().to_string()],
            standards: vec!["45001".into()],
            output_dir: Some(out.display().to_string()),
        });

        assert!(result.ok, "{:?}", result.log);
        assert!(!result.passed);
        assert_eq!(result.standards_labels, vec!["ISO 45001".to_string()]);
        assert_eq!(result.pass_message, "Compliancy passes: ISO 45001");
        assert!(!result.reasons.is_empty());
        assert!(result.reasons.iter().any(|r| r.contains("ISO 45001")));
        assert!(result.csv.contains("document,standard,clause,title,status"));
        assert!(result.report_text.contains("[MISS]") || result.report_text.contains("[PASS]"));
        assert!(result.combined_report.contains("--- Text report ---"));
        assert!(result.combined_report.contains("--- CSV ---"));
        assert!(result
            .files_written
            .iter()
            .any(|p| p.ends_with("iso-checklist.csv")));
        assert!(result
            .files_written
            .iter()
            .any(|p| p.ends_with("iso-checklist-report.txt")));
        // No transformed document output — checklist only.
        assert!(!result.files_written.iter().any(|p| p.ends_with(".docx")));
    }

    #[test]
    fn iso_run_first_passes_when_all_clauses_present() {
        let dir = std::env::temp_dir().join("compliancy-iso-run-first-pass");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Build a doc that hits every 45001 clause pattern via the clause id itself.
        let clauses = docx_core::clauses_for(&[docx_core::IsoStandard::Iso45001]);
        let mut paras: Vec<(&str, String)> = Vec::new();
        for c in &clauses {
            // Prefer a non-id pattern when available so ids don't false-match siblings.
            let needle = c
                .patterns
                .iter()
                .find(|p| p.chars().any(|ch| ch.is_ascii_alphabetic()))
                .copied()
                .unwrap_or(c.patterns[0]);
            paras.push(("body", format!("{} {}", c.clause, needle)));
        }
        let owned: Vec<(&str, &str)> = paras
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();
        let path = dir.join("full-45001.docx");
        write_minimal_docx(
            &path,
            "Heading1",
            "Normal",
            &owned,
            SAMPLE_STYLES,
            None,
        )
        .unwrap();

        let result = run_iso_first(IsoFirstRequest {
            input_paths: vec![path.display().to_string()],
            standards: vec!["45001".into()],
            output_dir: None,
        });

        assert!(result.ok, "{:?}", result.log);
        assert!(result.passed, "reasons: {:?}", result.reasons);
        assert!(result.reasons.is_empty());
        assert_eq!(result.pass_message, "Compliancy passes: ISO 45001");
        assert!(result.files_written.is_empty());
    }

    #[test]
    fn iso_run_first_requires_standards_and_inputs() {
        let no_std = run_iso_first(IsoFirstRequest {
            input_paths: vec!["/tmp/does-not-matter.docx".into()],
            standards: vec![],
            output_dir: None,
        });
        assert!(!no_std.ok);
        assert!(!no_std.passed);
        assert!(no_std
            .reasons
            .iter()
            .any(|r| r.contains("Select at least one ISO standard")));

        let no_in = run_iso_first(IsoFirstRequest {
            input_paths: vec![],
            standards: vec!["9001".into()],
            output_dir: None,
        });
        assert!(!no_in.ok);
        assert!(no_in
            .reasons
            .iter()
            .any(|r| r.contains("Choose input files")));
    }

    #[test]
    fn mixed_folder_match_input_and_summary_log() {
        let dir = std::env::temp_dir().join("compliancy-mixed-folder-run");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let _ = fixture_docx(&dir);
        write_minimal_docx(
            &dir.join("word-doc.docx"),
            "Heading1",
            "Normal",
            &[("body", "Word body text.")],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();
        pdf_core::write_pdf_from_blocks(
            &dir.join("pdf-doc.pdf"),
            &[("body", "PDF body for match-input.")],
            &pdf_core::PdfExportOptions {
                title: "In".into(),
                cues: pdf_core::PdfStyleCues::default(),
                apply_fonts: true,
                apply_colours: true,
                apply_background: false,
            },
        )
        .unwrap();
        fs::write(dir.join("legacy.doc"), b"\xd0\xcf\x11\xe0 not-real-ole").unwrap();
        fs::write(dir.join("readme.txt"), b"skip").unwrap();

        let out = dir.join("out");
        let result = run_pipeline(RunRequest {
            mode: "standard".into(),
            input_paths: vec![dir.display().to_string()],
            output_dir: out.display().to_string(),
            output_format: "match".into(),
            iso: IsoRuleset {
                enabled: false,
                standards: vec![],
            },
            template: TemplateRuleset {
                enabled: false,
                template_path: None,
                selected_trait_ids: None,
            },
            context: ContextRuleset {
                enabled: false,
                text: String::new(),
            },
            assist: None,
        });

        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Scan ") && l.message.contains(".docx=")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Skipped unsupported file") && l.message.contains("readme.txt")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Directory information:")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Successful output:")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Unsuccessful output:")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Skipped output:")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Total complete:")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Total fail:")));
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("Total file count in the output directory:")));

        assert!(result.files_written.iter().any(|p| p.ends_with("word-doc.docx")));
        assert!(result.files_written.iter().any(|p| p.ends_with("pdf-doc.pdf")));
        assert!(result.files_written.iter().any(|p| p.ends_with("sample.docx")));

        let doc_msgs: Vec<_> = result
            .log
            .iter()
            .filter(|l| l.message.to_lowercase().contains(".doc"))
            .map(|l| l.message.as_str())
            .collect();
        assert!(
            doc_msgs.iter().any(|m| m.contains("Unsuccessful")
                || m.contains("unavailable")
                || m.contains("conversion")
                || m.contains("Converted")),
            "expected .doc handling log, got: {doc_msgs:?}"
        );
    }

    #[test]
    fn multi_path_emits_overall_summary() {
        let dir = std::env::temp_dir().join("compliancy-multi-path-run");
        let _ = fs::remove_dir_all(&dir);
        let a = dir.join("a");
        let b = dir.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        write_minimal_docx(
            &a.join("one.docx"),
            "Heading1",
            "Normal",
            &[("body", "A")],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();
        write_minimal_docx(
            &b.join("two.docx"),
            "Heading1",
            "Normal",
            &[("body", "B")],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();
        let out = dir.join("out");
        let result = run_pipeline(RunRequest {
            mode: "standard".into(),
            input_paths: vec![a.display().to_string(), b.display().to_string()],
            output_dir: out.display().to_string(),
            output_format: "match".into(),
            iso: IsoRuleset {
                enabled: false,
                standards: vec![],
            },
            template: TemplateRuleset {
                enabled: false,
                template_path: None,
                selected_trait_ids: None,
            },
            context: ContextRuleset {
                enabled: false,
                text: String::new(),
            },
            assist: None,
        });
        assert!(result.ok, "{:?}", result.log);
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("OVERALL across")));
        assert_eq!(
            result
                .files_written
                .iter()
                .filter(|p| p.ends_with(".docx"))
                .count(),
            2
        );
    }

    #[test]
    fn iso_run_first_accepts_pdf_text() {
        let dir = std::env::temp_dir().join("compliancy-iso-pdf-first");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("partial.pdf");
        pdf_core::write_pdf_from_blocks(
            &pdf,
            &[
                ("heading", "4.1 Context of the organization"),
                ("body", "OH&S policy and worker consultation."),
            ],
            &pdf_core::PdfExportOptions {
                title: "ISO PDF".into(),
                cues: pdf_core::PdfStyleCues::default(),
                apply_fonts: true,
                apply_colours: true,
                apply_background: false,
            },
        )
        .unwrap();

        let result = run_iso_first(IsoFirstRequest {
            input_paths: vec![pdf.display().to_string()],
            standards: vec!["45001".into()],
            output_dir: None,
        });
        assert!(result.ok, "{:?}", result.log);
        assert!(!result.passed);
        assert!(result
            .log
            .iter()
            .any(|l| l.message.contains("PDF text limit")));
        assert!(!result.reasons.is_empty());
    }
}
