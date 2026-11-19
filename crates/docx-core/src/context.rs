//! Structured context rules (Standard mode — no LLM).
//!
//! Parses a small rule language from the Context UI field (line-based or JSON)
//! and applies it during the pipeline: force headings, find/replace, and
//! required-section presence checks beyond the ISO checklist.
//!
//! # Line syntax
//!
//! ```text
//! # comment
//! force_heading: Revision History
//! require_section: Document Control
//! require_heading: Scope
//! replace: Acme Works Ltd => ACME Works Limited
//!
//! # Optional per-document scope (basename match). `document: *` or bare
//! # `document:` returns to batch-global rules.
//! document: site-report.docx
//! replace: DRAFT => FINAL
//! ```
//!
//! # JSON syntax
//!
//! A document whose trimmed text starts with `{` is parsed as:
//!
//! ```json
//! {
//!   "rules": [
//!     {"op": "force_heading", "text": "Revision History"},
//!     {"op": "require_section", "text": "Document Control"},
//!     {"op": "replace", "find": "Acme", "with": "ACME Ltd"},
//!     {"op": "replace", "find": "DRAFT", "with": "FINAL", "document": "site.docx"}
//!   ]
//! }
//! ```

use std::fmt::Write as _;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde::Deserialize;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::extract::{extract_docx_text, extract_from_document_xml, DocText};
use crate::{read_zip_entry, xml_escape, DocxError, Result, DOCUMENT_PATH};

/// Kind of structured context rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOp {
    /// Ensure a Heading-styled paragraph with this text exists; append if missing.
    ForceHeading,
    /// Presence check: section/heading keywords must appear in document text.
    RequireSection,
    /// Presence check: preferably against extracted headings.
    RequireHeading,
    /// Literal find → replace inside `w:t` run text (per run; not cross-run).
    Replace,
}

impl RuleOp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ForceHeading => "force_heading",
            Self::RequireSection => "require_section",
            Self::RequireHeading => "require_heading",
            Self::Replace => "replace",
        }
    }
}

/// One parsed context rule. `document == None` ⇒ batch-global.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRule {
    pub op: RuleOp,
    /// Heading/section text, or find string for replace.
    pub text: String,
    /// Replacement string when `op == Replace`.
    pub replace_with: Option<String>,
    /// Optional basename scope (`report.docx`). `None` = all documents.
    pub document: Option<String>,
}

/// Outcome of applying / checking rules against one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    Applied,
    AlreadyPresent,
    Replaced { count: usize },
    Present,
    Missing,
    Skipped,
}

impl RuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::AlreadyPresent => "already_present",
            Self::Replaced { .. } => "replaced",
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuleFinding {
    pub document: String,
    pub op: RuleOp,
    pub detail: String,
    pub action: RuleAction,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ContextReport {
    pub findings: Vec<RuleFinding>,
}

impl ContextReport {
    pub fn missing_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.action == RuleAction::Missing)
            .count()
    }

    pub fn applied_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| {
                matches!(
                    f.action,
                    RuleAction::Applied | RuleAction::Replaced { .. }
                )
            })
            .count()
    }

    pub fn extend(&mut self, other: ContextReport) {
        self.findings.extend(other.findings);
    }

    pub fn to_text_report(&self) -> String {
        let mut out = String::new();
        out.push_str("Compliancy Checker — structured context rules\n");
        out.push_str(
            "Deterministic rule language only (no LLM). Presence / force / replace.\n",
        );
        let _ = writeln!(
            out,
            "Summary: {} applied/replaced, {} required-section misses\n",
            self.applied_count(),
            self.missing_count()
        );

        let mut current_doc: Option<&str> = None;
        for f in &self.findings {
            if current_doc != Some(f.document.as_str()) {
                current_doc = Some(&f.document);
                let _ = writeln!(out, "Document: {}", f.document);
            }
            let _ = writeln!(
                out,
                "  [{}] {} — {}",
                f.action.as_str(),
                f.op.as_str(),
                f.message
            );
        }
        out
    }

    pub fn to_csv(&self) -> String {
        let mut out = String::from("document,op,detail,action,message\n");
        for f in &self.findings {
            let action = match f.action {
                RuleAction::Replaced { count } => format!("replaced:{count}"),
                other => other.as_str().to_string(),
            };
            let _ = writeln!(
                out,
                "{},{},{},{},{}",
                csv_escape(&f.document),
                f.op.as_str(),
                csv_escape(&f.detail),
                csv_escape(&action),
                csv_escape(&f.message),
            );
        }
        out
    }
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Parse context field text into rules. Empty / whitespace ⇒ empty list.
pub fn parse_context_rules(raw: &str) -> Result<Vec<ContextRule>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('{') {
        return parse_json_rules(trimmed);
    }
    parse_line_rules(trimmed)
}

fn parse_line_rules(raw: &str) -> Result<Vec<ContextRule>> {
    let mut rules = Vec::new();
    let mut current_doc: Option<String> = None;

    for (lineno, line) in raw.lines().enumerate() {
        let line_no = lineno + 1;
        let mut line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((head, _rest)) = line.split_once('#') {
            // Allow `#` inside replace strings only when quoted — otherwise strip trailing comment.
            if !head.contains('"') && !head.contains('\'') {
                line = head.trim();
                if line.is_empty() {
                    continue;
                }
            }
        }

        let lower = line.to_ascii_lowercase();
        if lower.starts_with("document:") {
            let value = line["document:".len()..].trim();
            current_doc = if value.is_empty() || value == "*" {
                None
            } else {
                Some(value.to_string())
            };
            continue;
        }

        let rule = parse_line_rule(line, current_doc.clone()).map_err(|e| {
            DocxError::Message(format!("context rules line {line_no}: {e}"))
        })?;
        rules.push(rule);
    }
    Ok(rules)
}

fn parse_line_rule(line: &str, document: Option<String>) -> std::result::Result<ContextRule, String> {
    let (key, value) = split_key_value(line)
        .ok_or_else(|| format!("expected `op: value`, got `{line}`"))?;
    let key = key.to_ascii_lowercase();
    match key.as_str() {
        "force_heading" | "force_section" => {
            if value.is_empty() {
                return Err("force_heading requires text".into());
            }
            Ok(ContextRule {
                op: RuleOp::ForceHeading,
                text: value,
                replace_with: None,
                document,
            })
        }
        "require_section" => {
            if value.is_empty() {
                return Err("require_section requires text".into());
            }
            Ok(ContextRule {
                op: RuleOp::RequireSection,
                text: value,
                replace_with: None,
                document,
            })
        }
        "require_heading" => {
            if value.is_empty() {
                return Err("require_heading requires text".into());
            }
            Ok(ContextRule {
                op: RuleOp::RequireHeading,
                text: value,
                replace_with: None,
                document,
            })
        }
        "replace" => {
            let (find, with) = split_replace_value(&value)?;
            if find.is_empty() {
                return Err("replace find string must not be empty".into());
            }
            Ok(ContextRule {
                op: RuleOp::Replace,
                text: find,
                replace_with: Some(with),
                document,
            })
        }
        other => Err(format!(
            "unknown rule `{other}` (use force_heading, require_section, require_heading, replace, document)"
        )),
    }
}

fn split_key_value(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once(':')?;
    Some((k.trim().to_string(), v.trim().to_string()))
}

/// `old => new` or `"old" => "new"` / `'old' => 'new'`.
fn split_replace_value(value: &str) -> std::result::Result<(String, String), String> {
    let Some(idx) = value.find("=>") else {
        return Err("replace needs `find => replacement`".into());
    };
    let find = unquote(value[..idx].trim());
    let with = unquote(value[idx + 2..].trim());
    Ok((find, with))
}

fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let a = bytes[0];
        let b = bytes[bytes.len() - 1];
        if (a == b'"' && b == b'"') || (a == b'\'' && b == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

#[derive(Debug, Deserialize)]
struct JsonRoot {
    rules: Vec<JsonRule>,
}

#[derive(Debug, Deserialize)]
struct JsonRule {
    op: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    find: Option<String>,
    #[serde(default, rename = "with", alias = "replace")]
    with: Option<String>,
    #[serde(default)]
    document: Option<String>,
}

fn parse_json_rules(raw: &str) -> Result<Vec<ContextRule>> {
    let root: JsonRoot = serde_json::from_str(raw)
        .map_err(|e| DocxError::Message(format!("context rules JSON: {e}")))?;
    let mut out = Vec::new();
    for (i, jr) in root.rules.iter().enumerate() {
        let op_key = jr.op.trim().to_ascii_lowercase();
        let document = jr
            .document
            .as_ref()
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty() && d != "*");
        let rule = match op_key.as_str() {
            "force_heading" | "force_section" => {
                let text = jr
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        DocxError::Message(format!(
                            "context rules JSON rule {}: force_heading needs text",
                            i + 1
                        ))
                    })?
                    .to_string();
                ContextRule {
                    op: RuleOp::ForceHeading,
                    text,
                    replace_with: None,
                    document,
                }
            }
            "require_section" => {
                let text = jr
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        DocxError::Message(format!(
                            "context rules JSON rule {}: require_section needs text",
                            i + 1
                        ))
                    })?
                    .to_string();
                ContextRule {
                    op: RuleOp::RequireSection,
                    text,
                    replace_with: None,
                    document,
                }
            }
            "require_heading" => {
                let text = jr
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        DocxError::Message(format!(
                            "context rules JSON rule {}: require_heading needs text",
                            i + 1
                        ))
                    })?
                    .to_string();
                ContextRule {
                    op: RuleOp::RequireHeading,
                    text,
                    replace_with: None,
                    document,
                }
            }
            "replace" => {
                let find = jr
                    .find
                    .as_ref()
                    .or(jr.text.as_ref())
                    .map(|s| s.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        DocxError::Message(format!(
                            "context rules JSON rule {}: replace needs find",
                            i + 1
                        ))
                    })?
                    .to_string();
                let with = jr.with.clone().unwrap_or_default();
                ContextRule {
                    op: RuleOp::Replace,
                    text: find,
                    replace_with: Some(with),
                    document,
                }
            }
            other => {
                return Err(DocxError::Message(format!(
                    "context rules JSON rule {}: unknown op `{other}`",
                    i + 1
                )));
            }
        };
        out.push(rule);
    }
    Ok(out)
}

fn rule_applies(rule: &ContextRule, document_label: &str) -> bool {
    match &rule.document {
        None => true,
        Some(name) => {
            let want = name.trim();
            document_label.eq_ignore_ascii_case(want)
                || Path::new(document_label)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case(want))
                    .unwrap_or(false)
        }
    }
}

fn heading_present(text: &DocText, needle: &str) -> bool {
    let n = needle.to_lowercase();
    text.headings.iter().any(|h| h.to_lowercase().contains(&n))
}

fn section_present(text: &DocText, needle: &str) -> bool {
    let n = needle.to_lowercase();
    if text.search_blob().contains(&n) {
        return true;
    }
    heading_present(text, needle)
}

/// Apply transform rules and evaluate presence checks against a `.docx` path.
/// Mutates the package in place for `force_heading` / `replace`.
pub fn apply_context_to_docx(
    docx_path: &Path,
    rules: &[ContextRule],
) -> Result<ContextReport> {
    let label = docx_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("document.docx")
        .to_string();

    let applicable: Vec<&ContextRule> = rules
        .iter()
        .filter(|r| rule_applies(r, &label))
        .collect();

    if applicable.is_empty() {
        return Ok(ContextReport::default());
    }

    let bytes = std::fs::read(docx_path)?;
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let mut document_xml = read_zip_entry(&mut zip, DOCUMENT_PATH)?;

    // Collect other parts for rewrite.
    let mut parts: std::collections::HashMap<String, Vec<u8>> =
        std::collections::HashMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let name = file.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        parts.insert(name, data);
    }
    drop(zip);

    let mut findings = Vec::new();
    let mut dirty = false;

    // 1) Replaces first (so force/require see post-replace text).
    for rule in &applicable {
        if rule.op != RuleOp::Replace {
            continue;
        }
        let with = rule.replace_with.as_deref().unwrap_or("");
        let (new_xml, count) = replace_in_document_xml(&document_xml, &rule.text, with);
        if count > 0 {
            document_xml = new_xml;
            dirty = true;
        }
        findings.push(RuleFinding {
            document: label.clone(),
            op: RuleOp::Replace,
            detail: format!("{} => {}", rule.text, with),
            action: RuleAction::Replaced { count },
            message: if count == 0 {
                format!("no occurrences of `{}`", rule.text)
            } else {
                format!("replaced {count} occurrence(s) of `{}`", rule.text)
            },
        });
    }

    // 2) Force headings.
    for rule in &applicable {
        if rule.op != RuleOp::ForceHeading {
            continue;
        }
        let text_snap = extract_from_document_xml(&document_xml);
        if heading_present(&text_snap, &rule.text) || section_present(&text_snap, &rule.text) {
            findings.push(RuleFinding {
                document: label.clone(),
                op: RuleOp::ForceHeading,
                detail: rule.text.clone(),
                action: RuleAction::AlreadyPresent,
                message: format!("heading/section already present: {}", rule.text),
            });
        } else {
            document_xml = append_heading_paragraph(&document_xml, &rule.text)?;
            dirty = true;
            findings.push(RuleFinding {
                document: label.clone(),
                op: RuleOp::ForceHeading,
                detail: rule.text.clone(),
                action: RuleAction::Applied,
                message: format!("appended Heading1: {}", rule.text),
            });
        }
    }

    // 3) Presence checks on final XML.
    let final_text = extract_from_document_xml(&document_xml);
    for rule in &applicable {
        match rule.op {
            RuleOp::RequireSection => {
                let ok = section_present(&final_text, &rule.text);
                findings.push(RuleFinding {
                    document: label.clone(),
                    op: RuleOp::RequireSection,
                    detail: rule.text.clone(),
                    action: if ok {
                        RuleAction::Present
                    } else {
                        RuleAction::Missing
                    },
                    message: if ok {
                        format!("required section present: {}", rule.text)
                    } else {
                        format!("required section missing: {}", rule.text)
                    },
                });
            }
            RuleOp::RequireHeading => {
                let ok = heading_present(&final_text, &rule.text)
                    || section_present(&final_text, &rule.text);
                findings.push(RuleFinding {
                    document: label.clone(),
                    op: RuleOp::RequireHeading,
                    detail: rule.text.clone(),
                    action: if ok {
                        RuleAction::Present
                    } else {
                        RuleAction::Missing
                    },
                    message: if ok {
                        format!("required heading present: {}", rule.text)
                    } else {
                        format!("required heading missing: {}", rule.text)
                    },
                });
            }
            _ => {}
        }
    }

    if dirty {
        parts.insert(DOCUMENT_PATH.to_string(), document_xml.into_bytes());
        write_docx_parts(docx_path, &parts)?;
    }

    Ok(ContextReport { findings })
}

/// Evaluate require_* rules only (no mutation). Useful for pure checks.
pub fn evaluate_context_text(
    document_label: &str,
    text: &DocText,
    rules: &[ContextRule],
) -> ContextReport {
    let mut findings = Vec::new();
    for rule in rules {
        if !rule_applies(rule, document_label) {
            continue;
        }
        match rule.op {
            RuleOp::RequireSection => {
                let ok = section_present(text, &rule.text);
                findings.push(RuleFinding {
                    document: document_label.to_string(),
                    op: RuleOp::RequireSection,
                    detail: rule.text.clone(),
                    action: if ok {
                        RuleAction::Present
                    } else {
                        RuleAction::Missing
                    },
                    message: if ok {
                        format!("required section present: {}", rule.text)
                    } else {
                        format!("required section missing: {}", rule.text)
                    },
                });
            }
            RuleOp::RequireHeading => {
                let ok = heading_present(text, &rule.text) || section_present(text, &rule.text);
                findings.push(RuleFinding {
                    document: document_label.to_string(),
                    op: RuleOp::RequireHeading,
                    detail: rule.text.clone(),
                    action: if ok {
                        RuleAction::Present
                    } else {
                        RuleAction::Missing
                    },
                    message: if ok {
                        format!("required heading present: {}", rule.text)
                    } else {
                        format!("required heading missing: {}", rule.text)
                    },
                });
            }
            RuleOp::ForceHeading | RuleOp::Replace => {
                findings.push(RuleFinding {
                    document: document_label.to_string(),
                    op: rule.op,
                    detail: rule.text.clone(),
                    action: RuleAction::Skipped,
                    message: "transform rule skipped in text-only evaluate".into(),
                });
            }
        }
    }
    ContextReport { findings }
}

fn replace_in_document_xml(document_xml: &str, find: &str, with: &str) -> (String, usize) {
    if find.is_empty() {
        return (document_xml.to_string(), 0);
    }
    let find_esc = xml_escape(find);
    let with_esc = xml_escape(with);
    let mut count = 0usize;
    let mut out = String::with_capacity(document_xml.len());
    let mut rest = document_xml;

    while let Some(t_abs) = find_t_open(rest) {
        out.push_str(&rest[..t_abs]);
        let after_open = &rest[t_abs..];
        let Some(gt) = after_open.find('>') else {
            out.push_str(after_open);
            return (out, count);
        };
        // Self-closing <w:t .../>
        if after_open[..gt].ends_with('/')
            || after_open.as_bytes().get(gt.saturating_sub(1)) == Some(&b'/')
        {
            out.push_str(&after_open[..=gt]);
            rest = &after_open[gt + 1..];
            continue;
        }
        let content_start = gt + 1;
        let content = &after_open[content_start..];
        let Some(end) = content.find("</w:t>") else {
            out.push_str(after_open);
            return (out, count);
        };
        let text = &content[..end];
        let (new_text, n) = replace_all_count(text, &find_esc, &with_esc);
        count += n;
        out.push_str(&after_open[..content_start]);
        out.push_str(&new_text);
        out.push_str("</w:t>");
        rest = &content[end + "</w:t>".len()..];
    }
    out.push_str(rest);
    (out, count)
}

fn replace_all_count(hay: &str, find: &str, with: &str) -> (String, usize) {
    if find.is_empty() || !hay.contains(find) {
        return (hay.to_string(), 0);
    }
    let parts: Vec<&str> = hay.split(find).collect();
    let n = parts.len().saturating_sub(1);
    (parts.join(with), n)
}

fn find_t_open(xml: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(rel) = xml[search_from..].find("<w:t") {
        let absolute = search_from + rel;
        let after = absolute + "<w:t".len();
        let next = xml.as_bytes().get(after).copied();
        if next == Some(b'>') || next == Some(b' ') || next == Some(b'/') {
            return Some(absolute);
        }
        search_from = after;
    }
    None
}

fn append_heading_paragraph(document_xml: &str, heading_text: &str) -> Result<String> {
    let body_close = document_xml
        .rfind("</w:body>")
        .ok_or_else(|| DocxError::Message("missing </w:body>".into()))?;
    let before_close = &document_xml[..body_close];
    let after_close = &document_xml[body_close..];

    // Insert before trailing sectPr when present.
    let insert_at = if let Some(sect_start) = find_last_open_tag(before_close, "w:sectPr") {
        sect_start
    } else {
        before_close.len()
    };

    let para = format!(
        r#"<w:p>
  <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
  <w:r><w:t xml:space="preserve">{}</w:t></w:r>
</w:p>
"#,
        xml_escape(heading_text)
    );

    let mut out = String::with_capacity(document_xml.len() + para.len());
    out.push_str(&before_close[..insert_at]);
    out.push_str(&para);
    out.push_str(&before_close[insert_at..]);
    out.push_str(after_close);
    Ok(out)
}

fn find_last_open_tag(xml: &str, local: &str) -> Option<usize> {
    let open = format!("<{local}");
    xml.rfind(&open)
}

fn write_docx_parts(
    path: &Path,
    parts: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out_file = std::fs::File::create(path)?;
    let mut writer = ZipWriter::new(out_file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let mut names: Vec<_> = parts.keys().cloned().collect();
    names.sort();
    for name in names {
        let data = parts.get(&name).expect("key present");
        writer.start_file(name, options)?;
        writer.write_all(data)?;
    }
    writer.finish()?;
    Ok(())
}

/// Write context findings beside processed documents.
pub fn write_context_exports(
    report: &ContextReport,
    output_dir: &Path,
) -> Result<(String, String)> {
    std::fs::create_dir_all(output_dir)?;
    let csv_path = output_dir.join("context-rules.csv");
    let txt_path = output_dir.join("context-rules-report.txt");
    std::fs::write(&csv_path, report.to_csv())?;
    std::fs::write(&txt_path, report.to_text_report())?;
    Ok((
        csv_path.display().to_string(),
        txt_path.display().to_string(),
    ))
}

/// Convenience: extract + evaluate require rules without mutating.
pub fn evaluate_context_docx(
    docx_path: &Path,
    rules: &[ContextRule],
) -> Result<ContextReport> {
    let text = extract_docx_text(docx_path)?;
    let label = docx_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("document.docx");
    Ok(evaluate_context_text(label, &text, rules))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write_minimal_docx;
    use crate::SAMPLE_STYLES;
    use std::path::PathBuf;

    #[test]
    fn parses_line_rules_and_document_scope() {
        let raw = r#"
# batch
force_heading: Revision History
require_section: Document Control
replace: Acme Works Ltd => ACME Works Limited

document: site.docx
replace: DRAFT => FINAL
require_heading: Scope

document: *
force_heading: Approvals
"#;
        let rules = parse_context_rules(raw).unwrap();
        assert_eq!(rules.len(), 6);
        assert_eq!(rules[0].op, RuleOp::ForceHeading);
        assert_eq!(rules[0].text, "Revision History");
        assert!(rules[0].document.is_none());
        assert_eq!(rules[2].op, RuleOp::Replace);
        assert_eq!(rules[2].replace_with.as_deref(), Some("ACME Works Limited"));
        assert_eq!(rules[3].document.as_deref(), Some("site.docx"));
        assert_eq!(rules[3].text, "DRAFT");
        assert!(rules[5].document.is_none());
        assert_eq!(rules[5].text, "Approvals");
    }

    #[test]
    fn parses_json_rules() {
        let raw = r#"{
          "rules": [
            {"op": "force_heading", "text": "Revision History"},
            {"op": "require_section", "text": "Document Control"},
            {"op": "replace", "find": "Acme", "with": "ACME Ltd", "document": "a.docx"}
          ]
        }"#;
        let rules = parse_context_rules(raw).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[2].document.as_deref(), Some("a.docx"));
        assert_eq!(rules[2].replace_with.as_deref(), Some("ACME Ltd"));
    }

    #[test]
    fn rejects_unknown_line_op() {
        let err = parse_context_rules("magic: foo").unwrap_err().to_string();
        assert!(err.contains("unknown rule"));
    }

    #[test]
    fn apply_force_replace_and_require_on_docx() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-context-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let docx = dir.join("policy.docx");
        write_minimal_docx(
            &docx,
            "Heading1",
            "Normal",
            &[
                ("title", "Site Safety Report"),
                ("body", "Customer: Acme Works Ltd"),
                ("body", "Status: DRAFT pending review."),
            ],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();

        let rules = parse_context_rules(
            r#"
force_heading: Revision History
require_section: Revision History
require_heading: Missing Heading That Should Fail
replace: Acme Works Ltd => ACME Works Limited
replace: DRAFT => FINAL
"#,
        )
        .unwrap();

        let report = apply_context_to_docx(&docx, &rules).unwrap();
        assert!(report.applied_count() >= 1);
        assert_eq!(report.missing_count(), 1);

        let text = extract_docx_text(&docx).unwrap();
        assert!(text.plain.contains("ACME Works Limited"));
        assert!(!text.plain.contains("Acme Works Ltd"));
        assert!(text.plain.contains("FINAL"));
        assert!(!text.plain.contains("DRAFT"));
        assert!(
            text.headings.iter().any(|h| h.contains("Revision History"))
                || text.plain.contains("Revision History")
        );

        let (csv_path, txt_path) = write_context_exports(&report, &dir).unwrap();
        let csv = std::fs::read_to_string(csv_path).unwrap();
        let txt = std::fs::read_to_string(txt_path).unwrap();
        assert!(csv.contains("force_heading"));
        assert!(txt.contains("structured context rules"));
    }

    #[test]
    fn document_scope_skips_other_files() {
        let rules = parse_context_rules(
            r#"
document: only-me.docx
require_section: Secret Section
"#,
        )
        .unwrap();
        let text = DocText {
            plain: "nothing here".into(),
            headings: vec![],
        };
        let report = evaluate_context_text("other.docx", &text, &rules);
        assert!(report.findings.is_empty());

        let report2 = evaluate_context_text("only-me.docx", &text, &rules);
        assert_eq!(report2.missing_count(), 1);
    }
}
