//! High-level pipeline orchestration for the app shell.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use docx_core::{self, DocxError, IsoStandard};

use crate::assist::{
    self, interpret_freeform_context, iso_gap_narrative, looks_like_freeform_context, AssistConfig,
    AssistRuntime, AssistStatus,
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
    "docx".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// `standard` | `assist`
    pub mode: String,
    pub input_paths: Vec<String>,
    pub output_dir: String,
    /// `docx` | `pdf` — selectable save/output format (default `docx`).
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

    let inputs = match collect_docx_inputs(&req.input_paths) {
        Ok(v) if v.is_empty() => {
            log.push(log_line(
                "error",
                "No .docx inputs found for ISO RUN FIRST.",
            ));
            return IsoFirstResult {
                ok: false,
                passed: false,
                standards_labels,
                pass_message,
                reasons: vec!["No .docx inputs found for ISO RUN FIRST.".into()],
                csv: String::new(),
                report_text: String::new(),
                combined_report: String::new(),
                files_written,
                log,
            };
        }
        Ok(v) => v,
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

    log.push(log_line(
        "info",
        format!("Evaluating {} document(s)…", inputs.len()),
    ));

    let mut reports = Vec::new();
    let mut ok = true;
    for input in &inputs {
        match docx_core::evaluate_docx(input, &standards) {
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
                reports.push(report);
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

    let output_format = if req.output_format.eq_ignore_ascii_case("pdf") {
        "pdf"
    } else {
        "docx"
    };
    log.push(log_line(
        "info",
        format!("Output format: {output_format}"),
    ));

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
    let mut intermediate_docx: Vec<PathBuf> = Vec::new();

    for input in &inputs {
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let docx_name = format!("{stem}.docx");
        let docx_dest = output_dir.join(&docx_name);

        let result = if let Some(ref tpl) = merge_template {
            docx_core::transform_docx_with_options(input, tpl, &docx_dest, transform_opts)
        } else {
            fs::copy(input, &docx_dest)
                .map(|_| ())
                .map_err(DocxError::from)
        };

        match result {
            Ok(()) => {
                if output_format == "docx" {
                    log.push(log_line(
                        "info",
                        format!("Wrote {}", docx_dest.display()),
                    ));
                    files_written.push(docx_dest.display().to_string());
                } else {
                    intermediate_docx.push(docx_dest.clone());
                    log.push(log_line(
                        "info",
                        format!("Prepared intermediate {}", docx_dest.display()),
                    ));
                }
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
                    }
                    context_report.extend(report);
                }
                Err(e) => {
                    ok = false;
                    log.push(log_line(
                        "error",
                        format!("Context rules failed for {}: {e}", docx_dest.display()),
                    ));
                }
            }
        }

        if output_format == "pdf" {
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
            match pdf_core::write_pdf_from_docx(&docx_dest, &pdf_dest, &export_opts) {
                Ok(()) => {
                    log.push(log_line("info", format!("Wrote {}", pdf_dest.display())));
                    files_written.push(pdf_dest.display().to_string());
                    // Drop intermediate .docx when PDF is the selected output.
                    let _ = fs::remove_file(&docx_dest);
                }
                Err(e) => {
                    ok = false;
                    log.push(log_line(
                        "error",
                        format!("PDF export failed for {}: {e}", input.display()),
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

    let _ = intermediate_docx; // intermediates removed after PDF write
    if let Some(synth) = synthetic_template_path {
        let _ = fs::remove_file(synth);
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
}
