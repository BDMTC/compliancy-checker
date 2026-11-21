//! Open XML `.docx` helpers for Compliancy Checker.
//!
//! - Template style merge (transform spike)
//! - Plain-text / heading extraction
//! - ISO 9001 / 14001 / 45001 section presence checklist + CSV/report export
//! - Structured context rules (force heading, replace, require section)
//!
//! Transform strategy (Open XML):
//! 1. Start from the **template** package (styles, theme, fonts, media, headers/footers).
//! 2. Keep document-level `w:background` and body **chrome** from the template
//!    (behind-doc DrawingML / VML, drawing-only paragraphs) so page fills and
//!    background images survive.
//! 3. Extract body block-level children from the **source** `word/document.xml`
//!    (everything inside `<w:body>` except a trailing `<w:sectPr>`).
//! 4. Strip source **presentation** direct formatting (`w:color`, fonts, sizes, …)
//!    so per-style colours/typefaces from the template apply. Style IDs are kept —
//!    Heading styles keep their own colours; Normal/body uses the body style colour.
//!    We do **not** flatten the document to a single global colour.
//! 5. Keep the template's trailing `<w:sectPr>` (page setup / section properties).
//! 6. Write a new package under `output`.

mod context;
mod extract;
mod iso;
mod traits;

pub use context::{
    apply_context_to_docx, evaluate_context_docx, evaluate_context_text, parse_context_rules,
    write_context_exports, ContextReport, ContextRule, RuleAction, RuleFinding, RuleOp,
};
pub use extract::{extract_docx_text, extract_from_document_xml, DocText};
pub use iso::{
    clauses_for, evaluate_docx, evaluate_text, merge_reports, parse_standards, write_exports,
    ChecklistClause, ChecklistReport, Finding, IsoStandard, Presence,
};
pub use traits::{
    inspect_template, options_from_template, traits_by_kind, DetectedTrait, TemplatePreview,
    TemplatePreviewSummary, TraitKind, TraitSelection, TransformOptions,
};

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

#[derive(Debug, Error)]
pub enum DocxError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, DocxError>;

pub(crate) const DOCUMENT_PATH: &str = "word/document.xml";

/// Transform `source` using styles/structure from `template`, writing `output`.
///
/// Colour rule: preserve **per-style** (and mapped per-run style) colours from the
/// master template. Body text resolves through Normal/body; headings keep Heading*
/// colours. Source direct colour/font/size overrides are stripped so they cannot
/// flatten or mask those template definitions.
///
/// Applies **all** detected traits. Prefer [`transform_docx_with_options`] when the
/// UI/CLI picker has narrowed fonts / colours / backgrounds.
pub fn transform_docx(source: &Path, template: &Path, output: &Path) -> Result<()> {
    transform_docx_with_options(source, template, output, TransformOptions::all())
}

/// Like [`transform_docx`], but only applies the trait categories enabled in `opts`
/// (from the template preview picker: all / one / any subset).
pub fn transform_docx_with_options(
    source: &Path,
    template: &Path,
    output: &Path,
    opts: TransformOptions,
) -> Result<()> {
    let source_bytes = std::fs::read(source)?;
    let template_bytes = std::fs::read(template)?;

    let mut source_zip = ZipArchive::new(Cursor::new(source_bytes))?;
    let mut template_zip = ZipArchive::new(Cursor::new(template_bytes))?;

    let source_document = read_zip_entry(&mut source_zip, DOCUMENT_PATH)?;
    let mut template_document = read_zip_entry(&mut template_zip, DOCUMENT_PATH)?;

    if !opts.apply_background {
        template_document = traits::strip_document_background(&template_document);
    }

    let source_inner = extract_body_inner(&source_document)?;
    let source_inner = traits::strip_presentation_for_options(&source_inner, opts);
    let mut merged_document = if opts.apply_background {
        splice_body_content(&template_document, &source_inner)?
    } else {
        splice_body_content_without_chrome(&template_document, &source_inner)?
    };

    let mut parts: HashMap<String, Vec<u8>> = HashMap::new();
    for i in 0..template_zip.len() {
        let mut file = template_zip.by_index(i)?;
        let name = file.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        // Scrub unselected font/colour definitions out of styles parts so the
        // template package cannot re-introduce them via style IDs.
        let base = name.rsplit('/').next().unwrap_or(name.as_str());
        if base.starts_with("styles") && base.ends_with(".xml") {
            if let Ok(text) = std::str::from_utf8(&data) {
                data = traits::scrub_styles_for_options(text, opts).into_bytes();
            }
        }
        parts.insert(name, data);
    }

    // Belt-and-braces page fill from theme accent2 when document has no w:background.
    if opts.apply_background {
        if let Some(fill) = theme_accent2_hex(&parts) {
            merged_document = ensure_document_background(&merged_document, &fill);
        }
    } else {
        merged_document = traits::strip_document_background(&merged_document);
    }
    parts.insert(DOCUMENT_PATH.to_string(), merged_document.into_bytes());

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let out_file = File::create(output)?;
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

pub(crate) fn read_zip_entry<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    path: &str,
) -> Result<String> {
    let mut file = zip
        .by_name(path)
        .map_err(|_| DocxError::Message(format!("missing zip entry: {path}")))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(buf)
}

/// Return the XML inside `<w:body>...</w:body>`, excluding a trailing `<w:sectPr>...</w:sectPr>`.
fn extract_body_inner(document_xml: &str) -> Result<String> {
    let body_open = find_tag_end(document_xml, "w:body")
        .ok_or_else(|| DocxError::Message("missing <w:body>".into()))?;
    let body_close = document_xml
        .rfind("</w:body>")
        .ok_or_else(|| DocxError::Message("missing </w:body>".into()))?;

    let mut inner = document_xml[body_open..body_close].to_string();
    if let Some(sect_start) = find_last_open_tag(&inner, "w:sectPr") {
        if let Some(sect_end_rel) = find_element_end(&inner[sect_start..], "w:sectPr") {
            let sect_end = sect_start + sect_end_rel;
            inner = format!("{}{}", &inner[..sect_start], &inner[sect_end..]);
        }
    }
    Ok(inner.trim().to_string())
}

/// Replace template body content (except trailing sectPr) with `source_inner`.
///
/// Preserves template `w:background` via the document prefix. Leading behind-doc DrawingML
/// runs are **injected into the first source paragraph** (page-pinned, `behindDoc=1`) so they
/// cannot sit in a spacer paragraph that pushes text down. Template `wp:inline` photos are
/// never treated as chrome.
fn splice_body_content(template_document: &str, source_inner: &str) -> Result<String> {
    splice_body_content_inner(template_document, source_inner, true)
}

/// Same as [`splice_body_content`] but skips behind-doc chrome injection (background trait off).
fn splice_body_content_without_chrome(
    template_document: &str,
    source_inner: &str,
) -> Result<String> {
    splice_body_content_inner(template_document, source_inner, false)
}

fn splice_body_content_inner(
    template_document: &str,
    source_inner: &str,
    apply_chrome: bool,
) -> Result<String> {
    let body_open = find_tag_end(template_document, "w:body")
        .ok_or_else(|| DocxError::Message("template missing <w:body>".into()))?;
    let body_close = template_document
        .rfind("</w:body>")
        .ok_or_else(|| DocxError::Message("template missing </w:body>".into()))?;

    let template_inner = &template_document[body_open..body_close];
    let mut sect_pr = String::new();
    let mut inner_wo_sect = template_inner.to_string();
    if let Some(sect_start) = find_last_open_tag(template_inner, "w:sectPr") {
        if let Some(sect_end_rel) = find_element_end(&template_inner[sect_start..], "w:sectPr") {
            sect_pr = template_inner[sect_start..sect_start + sect_end_rel].to_string();
            inner_wo_sect = format!(
                "{}{}",
                &template_inner[..sect_start],
                &template_inner[sect_start + sect_end_rel..]
            );
        }
    }

    let body_content = if apply_chrome {
        let chrome_runs = extract_leading_behind_doc_runs(&inner_wo_sect);
        let chrome_runs = pin_anchors_behind_page(&chrome_runs);
        inject_runs_into_first_paragraph(source_inner, &chrome_runs)
    } else {
        source_inner.to_string()
    };

    let mut new_inner = String::new();
    new_inner.push('\n');
    new_inner.push_str(&body_content);
    new_inner.push('\n');
    if !sect_pr.is_empty() {
        new_inner.push_str(&sect_pr);
        new_inner.push('\n');
    }

    let mut out = String::with_capacity(template_document.len() + body_content.len());
    out.push_str(&template_document[..body_open]);
    out.push_str(&new_inner);
    out.push_str(&template_document[body_close..]);
    Ok(out)
}

/// Runs from leading behind-doc chrome paragraphs (until first meaningful text).
///
/// Skips `wp:inline` content photos. Returns raw `<w:r>…</w:r>` fragments to inject into
/// the first merged content paragraph — never a standalone empty chrome paragraph.
fn extract_leading_behind_doc_runs(template_body_inner: &str) -> String {
    let mut runs_out = String::new();
    let mut rest = template_body_inner;
    while let Some(start) = find_next_open_tag(rest, "w:p") {
        let from = &rest[start..];
        let Some(end) = find_element_end(from, "w:p") else {
            break;
        };
        let para = &from[..end];
        if paragraph_has_meaningful_text(para) {
            break;
        }
        if is_behind_doc_chrome(para) {
            let mut para_rest = para;
            while let Some(r_start) = find_next_open_tag(para_rest, "w:r") {
                let r_from = &para_rest[r_start..];
                let Some(r_end) = find_element_end(r_from, "w:r") else {
                    break;
                };
                let run = &r_from[..r_end];
                if run.contains("<w:drawing>")
                    || run.contains("<w:pict>")
                    || run.contains("AlternateContent")
                {
                    runs_out.push_str(run);
                }
                para_rest = &r_from[r_end..];
            }
        }
        rest = &from[end..];
    }
    runs_out
}

/// True only for floating anchors marked behind the document (not inline drawings).
fn is_behind_doc_chrome(para: &str) -> bool {
    if para.contains("behindDoc=\"1\"") {
        return true;
    }
    // VML fallback used by Word AlternateContent: negative z-index = behind text.
    para.contains("<w:pict>")
        && (para.contains("z-index:-") || para.contains("z-index: -"))
}

fn paragraph_has_meaningful_text(para: &str) -> bool {
    let mut rest = para;
    while let Some(start) = rest.find("<w:t") {
        let after = &rest[start..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let content_start = gt + 1;
        let Some(close) = after[content_start..].find("</w:t>") else {
            break;
        };
        let text = &after[content_start..content_start + close];
        if text.chars().any(|c| !c.is_whitespace()) {
            return true;
        }
        rest = &after[content_start + close + "</w:t>".len()..];
    }
    false
}

/// Force DrawingML anchors behind the page so they do not participate in paragraph flow.
fn pin_anchors_behind_page(chrome_runs: &str) -> String {
    if chrome_runs.is_empty() {
        return String::new();
    }
    let mut out = chrome_runs.to_string();

    // Ensure behindDoc on every floating anchor.
    out = out.replace("behindDoc=\"0\"", "behindDoc=\"1\"");
    // Insert behindDoc="1" on anchors that lack the attribute.
    out = ensure_anchor_attr(&out, "behindDoc", "1");

    // Page-relative placement (not paragraph/column — those leave a spacer para effect
    // when the anchor lived in its own empty paragraph before content).
    out = rewrite_position_relative_from(&out, "page");

    // Prefer wrapNone so the shape never reserves layout space.
    out = out.replace("<wp:wrapTopAndBottom/>", "<wp:wrapNone/>");
    out = out.replace("<wp:wrapSquare/>", "<wp:wrapNone/>");
    out = out.replace("<wp:wrapThrough/>", "<wp:wrapNone/>");

    // VML fallback: keep negative z-index; prefer page-relative hints when present.
    out = out.replace(
        "mso-position-horizontal-relative:text",
        "mso-position-horizontal-relative:page",
    );
    out = out.replace(
        "mso-position-vertical-relative:text",
        "mso-position-vertical-relative:page",
    );
    out = out.replace(
        "mso-position-horizontal-relative:char",
        "mso-position-horizontal-relative:page",
    );
    out = out.replace(
        "mso-position-vertical-relative:line",
        "mso-position-vertical-relative:page",
    );

    out
}

fn ensure_anchor_attr(xml: &str, attr: &str, value: &str) -> String {
    let mut out = String::with_capacity(xml.len() + 64);
    let mut rest = xml;
    let needle = "<wp:anchor";
    while let Some(at) = rest.find(needle) {
        out.push_str(&rest[..at]);
        let from = &rest[at..];
        let Some(gt) = from.find('>') else {
            out.push_str(from);
            return out;
        };
        let open = &from[..=gt];
        if open.contains(&format!("{attr}=")) {
            out.push_str(open);
        } else {
            // Insert before closing `>` or `/>`
            let insert_at = if open.ends_with("/>") {
                open.len() - 2
            } else {
                open.len() - 1
            };
            out.push_str(&open[..insert_at]);
            out.push(' ');
            out.push_str(attr);
            out.push_str("=\"");
            out.push_str(value);
            out.push('"');
            out.push_str(&open[insert_at..]);
        }
        rest = &from[gt + 1..];
    }
    out.push_str(rest);
    out
}

fn rewrite_position_relative_from(xml: &str, relative: &str) -> String {
    let mut out = xml.to_string();
    for tag in ["wp:positionH", "wp:positionV"] {
        // Replace relativeFrom="…" inside these elements' opening tags.
        let open = format!("<{tag}");
        let mut rebuilt = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(&open) {
            rebuilt.push_str(&rest[..at]);
            let from = &rest[at..];
            let Some(gt) = from.find('>') else {
                rebuilt.push_str(from);
                out = rebuilt;
                return out;
            };
            let mut open_tag = from[..=gt].to_string();
            if let Some(rf) = open_tag.find("relativeFrom=\"") {
                let val_start = rf + "relativeFrom=\"".len();
                if let Some(val_end) = open_tag[val_start..].find('"') {
                    let end = val_start + val_end;
                    open_tag.replace_range(val_start..end, relative);
                }
            } else {
                let insert_at = open_tag.len() - 1;
                open_tag.insert_str(insert_at, &format!(" relativeFrom=\"{relative}\""));
            }
            rebuilt.push_str(&open_tag);
            rest = &from[gt + 1..];
        }
        rebuilt.push_str(rest);
        out = rebuilt;
    }
    // Zero page offsets so the fill starts at the page origin.
    out = replace_pos_offset_in_positions(&out);
    out
}

fn replace_pos_offset_in_positions(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(at) = rest.find("<wp:posOffset>") {
        out.push_str(&rest[..at]);
        let from = &rest[at..];
        if let Some(end) = from.find("</wp:posOffset>") {
            out.push_str("<wp:posOffset>0</wp:posOffset>");
            rest = &from[end + "</wp:posOffset>".len()..];
        } else {
            out.push_str(from);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Insert `<w:background w:color="…"/>` before `<w:body>` when missing.
fn ensure_document_background(document_xml: &str, color_hex: &str) -> String {
    if document_xml.contains("<w:background") {
        return document_xml.to_string();
    }
    let color = color_hex.trim_start_matches('#').to_uppercase();
    if let Some(at) = document_xml.find("<w:body") {
        return format!(
            "{}<w:background w:color=\"{color}\"/>{}",
            &document_xml[..at],
            &document_xml[at..]
        );
    }
    document_xml.to_string()
}

fn theme_accent2_hex(parts: &HashMap<String, Vec<u8>>) -> Option<String> {
    for (name, data) in parts {
        if !(name.starts_with("word/theme/") && name.ends_with(".xml")) {
            continue;
        }
        let Ok(theme) = std::str::from_utf8(data) else {
            continue;
        };
        if let Some(block_start) = theme.find("<a:accent2>") {
            let block = &theme[block_start..];
            if let Some(end) = block.find("</a:accent2>") {
                let inner = &block[..end];
                if let Some(val_at) = inner.find("val=\"") {
                    let v = &inner[val_at + 5..];
                    if let Some(q) = v.find('"') {
                        return Some(v[..q].to_string());
                    }
                }
            }
        }
    }
    None
}

/// Insert chrome runs after the first paragraph's `w:pPr` (or at start of the para body).
fn inject_runs_into_first_paragraph(body: &str, runs: &str) -> String {
    if runs.is_empty() {
        return body.to_string();
    }
    if let Some(start) = find_next_open_tag(body, "w:p") {
        let from = &body[start..];
        if let Some(end) = find_element_end(from, "w:p") {
            let para = &from[..end];
            let insert_at = if let Some(ppr_rel) = find_next_open_tag(para, "w:pPr") {
                let ppr_from = &para[ppr_rel..];
                match find_element_end(ppr_from, "w:pPr") {
                    Some(ppr_end) => ppr_rel + ppr_end,
                    None => para.find('>').map(|i| i + 1).unwrap_or(0),
                }
            } else {
                para.find('>').map(|i| i + 1).unwrap_or(0)
            };
            let new_para = format!("{}{}{}", &para[..insert_at], runs, &para[insert_at..]);
            return format!("{}{}{}", &body[..start], new_para, &from[end..]);
        }
    }
    format!("<w:p>{runs}</w:p>\n{body}")
}

fn find_next_open_tag(xml: &str, local: &str) -> Option<usize> {
    let open = format!("<{local}");
    let mut search_from = 0;
    while let Some(rel) = xml[search_from..].find(&open) {
        let at = search_from + rel;
        let after_name = at + open.len();
        let next = xml.as_bytes().get(after_name).copied();
        // Ensure we matched the element name boundary (`<w:p ` / `<w:p>`), not `<w:pPr` etc.
        if next == Some(b'>') || next == Some(b' ') || next == Some(b'/') || next == Some(b'\n') || next == Some(b'\r') || next == Some(b'\t')
        {
            return Some(at);
        }
        search_from = after_name;
    }
    None
}

fn find_tag_end(xml: &str, local: &str) -> Option<usize> {
    let open = format!("<{local}");
    let start = xml.find(&open)?;
    let after = xml[start..].find('>')? + start + 1;
    Some(after)
}

fn find_last_open_tag(xml: &str, local: &str) -> Option<usize> {
    let open = format!("<{local}");
    xml.rfind(&open)
}

/// Given a slice starting at `<local ...`, return the byte length through the matching end.
fn find_element_end(xml: &str, local: &str) -> Option<usize> {
    let first_gt = xml.find('>')?;
    // Self-closing
    if xml[..first_gt].ends_with('/') || xml.as_bytes().get(first_gt.saturating_sub(1)) == Some(&b'/')
    {
        return Some(first_gt + 1);
    }
    let close = format!("</{local}>");
    let close_at = xml.find(&close)?;
    Some(close_at + close.len())
}

/// Build a minimal `.docx` from plain text lines (PDF→Word / text materialisation).
pub fn write_docx_from_plain_lines(path: &Path, lines: &[String]) -> Result<()> {
    let owned: Vec<(String, String)> = if lines.iter().all(|l| l.trim().is_empty()) {
        vec![("body".into(), "(empty document)".into())]
    } else {
        lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| ("body".to_string(), l.clone()))
            .collect()
    };
    let refs: Vec<(&str, &str)> = owned
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    write_minimal_docx(path, "Heading1", "Normal", &refs, SAMPLE_STYLES, None)
}

/// Build a minimal but valid `.docx` (Office Open XML) package.
pub fn write_minimal_docx(
    path: &Path,
    title_style_id: &str,
    body_style_id: &str,
    paragraphs: &[(&str, &str)],
    styles_xml: &str,
    theme_xml: Option<&str>,
) -> Result<()> {
    let mut body = String::new();
    for (style, text) in paragraphs {
        let style_id = if *style == "title" {
            title_style_id
        } else {
            body_style_id
        };
        body.push_str(&paragraph_xml(style_id, text));
    }
    body.push_str(
        r#"<w:sectPr>
  <w:pgSz w:w="12240" w:h="15840"/>
  <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>
</w:sectPr>"#,
    );

    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
{body}
  </w:body>
</w:document>"#
    );

    write_package_with_document(path, &document_xml, styles_xml, theme_xml)
}

fn paragraph_xml(style_id: &str, text: &str) -> String {
    let escaped = xml_escape(text);
    format!(
        r#"<w:p>
  <w:pPr><w:pStyle w:val="{style_id}"/></w:pPr>
  <w:r><w:t xml:space="preserve">{escaped}</w:t></w:r>
</w:p>
"#
    )
}

pub(crate) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub const SAMPLE_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
    <w:qFormat/>
    <w:rPr>
      <w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/>
      <w:sz w:val="24"/>
      <w:color w:val="000000"/>
    </w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:qFormat/>
    <w:rPr>
      <w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/>
      <w:b/>
      <w:sz w:val="32"/>
      <w:color w:val="000000"/>
    </w:rPr>
  </w:style>
</w:styles>"#;

/// Style cues used when synthesising a master `.docx` from a non-Word template (e.g. PDF).
#[derive(Debug, Clone)]
pub struct SyntheticStyleCues {
    pub body_font: String,
    pub heading_font: String,
    pub body_colour: String,
    pub heading_colour: String,
    /// Page width in twips (1/20 pt). Default letter ≈ 12240.
    pub page_width_twips: u32,
    /// Page height in twips. Default letter ≈ 15840.
    pub page_height_twips: u32,
    /// Optional document background fill (`#` omitted).
    pub background_fill: Option<String>,
}

impl Default for SyntheticStyleCues {
    fn default() -> Self {
        Self {
            body_font: "Calibri".into(),
            heading_font: "Calibri".into(),
            body_colour: "1F4E79".into(),
            heading_colour: "C45911".into(),
            page_width_twips: 12240,
            page_height_twips: 15840,
            background_fill: None,
        }
    }
}

/// Write a minimal master template `.docx` from style cues (PDF → Word bridge).
///
/// Body is a single placeholder paragraph so the package is valid for
/// [`transform_docx_with_options`]; styles/theme/page size carry the cues.
pub fn write_synthetic_style_template(path: &Path, cues: &SyntheticStyleCues) -> Result<()> {
    let body_font = xml_escape(&cues.body_font);
    let heading_font = xml_escape(&cues.heading_font);
    let body_colour = normalize_hex_colour(&cues.body_colour).unwrap_or_else(|| "1F4E79".into());
    let heading_colour =
        normalize_hex_colour(&cues.heading_colour).unwrap_or_else(|| "C45911".into());
    let fill = cues
        .background_fill
        .as_deref()
        .and_then(normalize_hex_colour);

    let styles_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
    <w:qFormat/>
    <w:rPr>
      <w:rFonts w:ascii="{body_font}" w:hAnsi="{body_font}"/>
      <w:sz w:val="22"/>
      <w:color w:val="{body_colour}"/>
    </w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:qFormat/>
    <w:rPr>
      <w:rFonts w:ascii="{heading_font}" w:hAnsi="{heading_font}"/>
      <w:b/>
      <w:sz w:val="36"/>
      <w:color w:val="{heading_colour}"/>
    </w:rPr>
  </w:style>
</w:styles>"#
    );

    let bg = fill
        .as_ref()
        .map(|h| format!(r#"  <w:background w:color="{h}"/>"#))
        .unwrap_or_default();

    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
{bg}
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Normal"/></w:pPr>
      <w:r><w:t xml:space="preserve">Synthetic template</w:t></w:r>
    </w:p>
    <w:sectPr>
      <w:pgSz w:w="{w}" w:h="{h}"/>
      <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>
    </w:sectPr>
  </w:body>
</w:document>"#,
        w = cues.page_width_twips,
        h = cues.page_height_twips,
    );

    let theme = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Synthetic">
  <a:themeElements>
    <a:clrScheme name="Synthetic">
      <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
      <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="{body_colour}"/></a:dk2>
      <a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
      <a:accent1><a:srgbClr val="{body_colour}"/></a:accent1>
      <a:accent2><a:srgbClr val="{heading_colour}"/></a:accent2>
      <a:accent3><a:srgbClr val="9BBB59"/></a:accent3>
      <a:accent4><a:srgbClr val="8064A2"/></a:accent4>
      <a:accent5><a:srgbClr val="4BACC6"/></a:accent5>
      <a:accent6><a:srgbClr val="F79646"/></a:accent6>
      <a:hlink><a:srgbClr val="0000FF"/></a:hlink>
      <a:folHlink><a:srgbClr val="800080"/></a:folHlink>
    </a:clrScheme>
    <a:fontScheme name="Synthetic">
      <a:majorFont>
        <a:latin typeface="{heading_font}"/>
        <a:ea typeface=""/>
        <a:cs typeface=""/>
      </a:majorFont>
      <a:minorFont>
        <a:latin typeface="{body_font}"/>
        <a:ea typeface=""/>
        <a:cs typeface=""/>
      </a:minorFont>
    </a:fontScheme>
    <a:fmtScheme name="Office">
      <a:fillStyleLst>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
      </a:fillStyleLst>
      <a:lnStyleLst>
        <a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
        <a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
        <a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
      </a:lnStyleLst>
      <a:effectStyleLst>
        <a:effectStyle><a:effectLst/></a:effectStyle>
        <a:effectStyle><a:effectLst/></a:effectStyle>
        <a:effectStyle><a:effectLst/></a:effectStyle>
      </a:effectStyleLst>
      <a:bgFillStyleLst>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
      </a:bgFillStyleLst>
    </a:fmtScheme>
  </a:themeElements>
</a:theme>"#
    );

    write_package_with_document(path, &document_xml, &styles_xml, Some(&theme))
}

fn normalize_hex_colour(raw: &str) -> Option<String> {
    let s = raw.trim().trim_start_matches('#').to_ascii_uppercase();
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(s)
    } else {
        None
    }
}

pub const TEMPLATE_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
    <w:qFormat/>
    <w:rPr>
      <w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/>
      <w:sz w:val="22"/>
      <w:color w:val="1F4E79"/>
    </w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:qFormat/>
    <w:rPr>
      <w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/>
      <w:b/>
      <w:sz w:val="36"/>
      <w:color w:val="C45911"/>
    </w:rPr>
  </w:style>
</w:styles>"#;

const DEFAULT_THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme">
  <a:themeElements>
    <a:clrScheme name="Office">
      <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
      <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="1F497D"/></a:dk2>
      <a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
      <a:accent1><a:srgbClr val="4F81BD"/></a:accent1>
      <a:accent2><a:srgbClr val="C0504D"/></a:accent2>
      <a:accent3><a:srgbClr val="9BBB59"/></a:accent3>
      <a:accent4><a:srgbClr val="8064A2"/></a:accent4>
      <a:accent5><a:srgbClr val="4BACC6"/></a:accent5>
      <a:accent6><a:srgbClr val="F79646"/></a:accent6>
      <a:hlink><a:srgbClr val="0000FF"/></a:hlink>
      <a:folHlink><a:srgbClr val="800080"/></a:folHlink>
    </a:clrScheme>
    <a:fontScheme name="Office">
      <a:majorFont>
        <a:latin typeface="Calibri"/>
        <a:ea typeface=""/>
        <a:cs typeface=""/>
      </a:majorFont>
      <a:minorFont>
        <a:latin typeface="Calibri"/>
        <a:ea typeface=""/>
        <a:cs typeface=""/>
      </a:minorFont>
    </a:fontScheme>
    <a:fmtScheme name="Office">
      <a:fillStyleLst>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
      </a:fillStyleLst>
      <a:lnStyleLst>
        <a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
        <a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
        <a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
      </a:lnStyleLst>
      <a:effectStyleLst>
        <a:effectStyle><a:effectLst/></a:effectStyle>
        <a:effectStyle><a:effectLst/></a:effectStyle>
        <a:effectStyle><a:effectLst/></a:effectStyle>
      </a:effectStyleLst>
      <a:bgFillStyleLst>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
      </a:bgFillStyleLst>
    </a:fmtScheme>
  </a:themeElements>
</a:theme>"#;

/// Like [`write_minimal_docx`], but each run carries direct black colour / TNR formatting
/// (simulates Word source docs whose `w:rPr` would otherwise mask template style colours).
pub fn write_docx_with_direct_formatting(
    path: &Path,
    title_style_id: &str,
    body_style_id: &str,
    paragraphs: &[(&str, &str)],
    styles_xml: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut body = String::new();
    for (style, text) in paragraphs {
        let style_id = if *style == "title" {
            title_style_id
        } else {
            body_style_id
        };
        let escaped = xml_escape(text);
        body.push_str(&format!(
            r#"<w:p>
  <w:pPr>
    <w:pStyle w:val="{style_id}"/>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/><w:sz w:val="24"/><w:color w:val="000000"/></w:rPr>
  </w:pPr>
  <w:r>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/><w:sz w:val="24"/><w:color w:val="000000"/></w:rPr>
    <w:t xml:space="preserve">{escaped}</w:t>
  </w:r>
</w:p>
"#
        ));
    }
    body.push_str(
        r#"<w:sectPr>
  <w:pgSz w:w="12240" w:h="15840"/>
  <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>
</w:sectPr>"#,
    );

    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
{body}
  </w:body>
</w:document>"#
    );

    // Reuse package shell from write_minimal_docx by writing styles + theme the same way.
    write_package_with_document(path, &document_xml, styles_xml, None)
}

fn write_package_with_document(
    path: &Path,
    document_xml: &str,
    styles_xml: &str,
    theme_xml: Option<&str>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
  <Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
</Types>"#;

    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>
</Relationships>"#;

    let theme = theme_xml.unwrap_or(DEFAULT_THEME);

    let out_file = File::create(path)?;
    let mut writer = ZipWriter::new(out_file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let entries = [
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels),
        ("word/document.xml", document_xml),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/styles.xml", styles_xml),
        ("word/theme/theme1.xml", theme),
    ];

    for (name, data) in entries {
        writer.start_file(name, options)?;
        writer.write_all(data.as_bytes())?;
    }
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn read_out(path: &Path) -> (String, HashMap<String, String>, Vec<String>) {
        let out_bytes = std::fs::read(path).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(out_bytes)).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .filter(|n| !n.ends_with('/'))
            .collect();
        let mut parts = HashMap::new();
        for name in &names {
            if let Ok(s) = read_zip_entry(&mut zip, name) {
                parts.insert(name.clone(), s);
            }
        }
        let document = parts.get(DOCUMENT_PATH).cloned().unwrap_or_default();
        (document, parts, names)
    }

    #[test]
    fn transform_preserves_source_text_and_uses_template_styles() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let source = dir.join("source.docx");
        let template = dir.join("template.docx");
        let output = dir.join("out.docx");

        write_minimal_docx(
            &source,
            "Heading1",
            "Normal",
            &[
                ("title", "Site Safety Report"),
                ("body", "Customer: Acme Works Ltd"),
                ("body", "Address: 12 Harbour Road, Bristol BS1 1AA"),
            ],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();

        write_minimal_docx(
            &template,
            "Heading1",
            "Normal",
            &[("title", "TEMPLATE PLACEHOLDER"), ("body", "replace me")],
            TEMPLATE_STYLES,
            None,
        )
        .unwrap();

        transform_docx(&source, &template, &output).unwrap();

        let (document, parts, _) = read_out(&output);
        let styles = parts.get("word/styles.xml").unwrap();

        assert!(document.contains("Acme Works Ltd"));
        assert!(document.contains("12 Harbour Road, Bristol BS1 1AA"));
        assert!(!document.contains("TEMPLATE PLACEHOLDER"));
        assert!(styles.contains("Calibri"));
        assert!(styles.contains("C45911"));
        assert!(styles.contains("1F4E79"));
        assert!(!styles.contains("Times New Roman"));
    }

    #[test]
    fn transform_strips_source_direct_colour_so_per_style_colours_apply() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-test-colour");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let source = dir.join("source.docx");
        let template = dir.join("template.docx");
        let output = dir.join("out.docx");

        write_docx_with_direct_formatting(
            &source,
            "Heading1",
            "Normal",
            &[
                ("title", "Heading stays orange via style"),
                ("body", "Body stays navy via style"),
            ],
            SAMPLE_STYLES,
        )
        .unwrap();

        write_minimal_docx(
            &template,
            "Heading1",
            "Normal",
            &[("title", "TEMPLATE"), ("body", "x")],
            TEMPLATE_STYLES,
            None,
        )
        .unwrap();

        transform_docx(&source, &template, &output).unwrap();
        let (document, parts, _) = read_out(&output);
        let styles = parts.get("word/styles.xml").unwrap();

        assert!(document.contains("Heading stays orange via style"));
        assert!(document.contains("Body stays navy via style"));
        // Direct black from source must not remain on runs/paragraph marks.
        assert!(
            !document.contains("<w:color"),
            "source direct <w:color> should be stripped; colours come from template styles"
        );
        assert!(!document.contains("Times New Roman"));
        // Distinct per-style colours remain in the template styles part (not flattened).
        assert!(styles.contains("C45911"), "Heading1 colour");
        assert!(styles.contains("1F4E79"), "Normal/body colour");
        assert!(document.contains("w:pStyle w:val=\"Heading1\""));
        assert!(document.contains("w:pStyle w:val=\"Normal\""));
    }

    #[test]
    fn transform_keeps_template_background_and_media_from_real_structure_fixture() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures");
        let template = fixtures.join("master-template-example.docx");
        assert!(
            template.exists(),
            "missing slimmed Beau master fixture at {}",
            template.display()
        );

        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-test-bg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.docx");
        let output = dir.join("out.docx");

        write_docx_with_direct_formatting(
            &source,
            "Heading1",
            "Normal",
            &[
                ("title", "Site Safety Report"),
                ("body", "Customer: Acme Works Ltd — factual body preserved."),
            ],
            SAMPLE_STYLES,
        )
        .unwrap();

        transform_docx(&source, &template, &output).unwrap();
        let (document, parts, names) = read_out(&output);

        assert!(document.contains("Acme Works Ltd"));
        assert!(!document.contains("MASTER TEMPLATE PLACEHOLDER"));
        assert!(
            document.contains("behindDoc=\"1\""),
            "template behind-doc DrawingML chrome must survive"
        );
        assert!(
            document.contains("<w:background"),
            "document w:background fill must survive"
        );
        assert!(
            document.contains("r:embed=") || document.contains("a:blip"),
            "background image relationship/blip must survive"
        );
        assert!(
            !document.contains("<wp:inline"),
            "template inline photos must not be prepended as chrome (pushes text / sits in front)"
        );
        // Chrome is injected into the first text paragraph (no leading spacer).
        assert!(
            document.contains("relativeFrom=\"page\""),
            "behind-doc chrome must be page-pinned"
        );
        assert!(
            names.iter().any(|k| k.starts_with("word/media/")),
            "template media part must remain in package"
        );

        // Per-style colours from real Word styles2.xml + theme (not a single global colour).
        let styles = parts
            .get("word/styles2.xml")
            .expect("real Word packages may use styles2.xml");
        assert!(styles.contains("068145"), "Heading1 / accent green");
        assert!(
            styles.contains("themeColor=\"text1\"") || styles.contains("000000"),
            "Normal/body colour distinct from heading"
        );
        let theme = parts
            .get("word/theme/theme11.xml")
            .expect("theme11.xml from master");
        assert!(theme.contains("068145") || theme.contains("Catering Colors"));
        // Source direct `<w:color…/>` stripped (document `w:background w:color=` may remain).
        assert!(
            !document.contains("<w:color"),
            "run/paragraph direct <w:color> overrides must be stripped"
        );
    }

    #[test]
    fn strip_presentation_keeps_bold_and_styles() {
        let xml = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:rPr><w:b/><w:color w:val="000000"/><w:sz w:val="24"/></w:rPr></w:pPr><w:r><w:rPr><w:b/><w:i/><w:color w:val="FF0000"/><w:rFonts w:ascii="Arial"/></w:rPr><w:t>Hi</w:t></w:r></w:p>"#;
        let out = traits::strip_presentation_for_options(xml, TransformOptions::all());
        assert!(out.contains("w:pStyle w:val=\"Heading1\""));
        assert!(out.contains("<w:b/>"));
        assert!(out.contains("<w:i/>"));
        assert!(!out.contains("w:color"));
        assert!(!out.contains("w:rFonts"));
        assert!(!out.contains("w:sz"));
        assert!(out.contains(">Hi<"));
    }

    #[test]
    fn chrome_runs_skip_inline_and_mid_doc_and_pin_to_page() {
        let body = r#"
<w:p><w:pPr><w:spacing w:before="240"/></w:pPr><w:r><w:drawing><wp:anchor behindDoc="1" relativeHeight="1"><wp:positionH relativeFrom="column"><wp:posOffset>-10</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>-20</wp:posOffset></wp:positionV><wp:wrapNone/></wp:anchor></w:drawing></w:r></w:p>
<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1" cy="1"/></wp:inline></w:drawing></w:r></w:p>
<w:p><w:r><w:t>Who we are</w:t></w:r></w:p>
<w:p><w:r><w:drawing><wp:anchor behindDoc="1" relativeHeight="2"><wp:wrapNone/></wp:anchor></w:drawing></w:r></w:p>
"#;
        let runs = extract_leading_behind_doc_runs(body);
        assert!(runs.contains("behindDoc=\"1\""));
        assert!(runs.contains("relativeHeight=\"1\""));
        assert!(!runs.contains("<wp:inline"), "inline photos are not chrome");
        assert!(
            !runs.contains("relativeHeight=\"2\""),
            "mid-doc behindDoc after text must not be taken"
        );
        assert!(!runs.contains("Who we are"));
        assert!(!runs.contains("<w:p"), "chrome must be runs, not a spacer paragraph");

        let pinned = pin_anchors_behind_page(&runs);
        assert!(pinned.contains("relativeFrom=\"page\""));
        assert!(!pinned.contains("relativeFrom=\"paragraph\""));
        assert!(!pinned.contains("relativeFrom=\"column\""));
        assert!(pinned.contains("<wp:posOffset>0</wp:posOffset>"));

        let injected = inject_runs_into_first_paragraph(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>"#,
            &pinned,
        );
        assert!(injected.contains("behindDoc=\"1\""));
        assert!(injected.contains(">Title<"));
        // Single paragraph: chrome + text together (no leading empty para).
        let mut para_count = 0;
        let mut rest = injected.as_str();
        while let Some(start) = find_next_open_tag(rest, "w:p") {
            para_count += 1;
            let from = &rest[start..];
            let end = find_element_end(from, "w:p").unwrap();
            rest = &from[end..];
        }
        assert_eq!(para_count, 1);
    }

    #[test]
    fn transform_with_options_can_skip_background_and_keep_source_fonts() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures");
        let template = fixtures.join("master-template-example.docx");
        assert!(template.exists());

        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-test-traits-apply");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.docx");
        let output = dir.join("out.docx");

        write_docx_with_direct_formatting(
            &source,
            "Heading1",
            "Normal",
            &[
                ("title", "Selective Traits Report"),
                ("body", "Customer: Acme Works Ltd"),
            ],
            SAMPLE_STYLES,
        )
        .unwrap();

        let preview = inspect_template(&template).unwrap();
        let colour_ids: Vec<String> = preview
            .traits
            .iter()
            .filter(|t| t.kind == TraitKind::Colour)
            .map(|t| t.id.clone())
            .collect();
        assert!(!colour_ids.is_empty());

        let opts = TransformOptions::from_selection(
            &preview.traits,
            &TraitSelection {
                selected_ids: Some(colour_ids),
            },
        );
        assert!(opts.apply_colours);
        assert!(!opts.apply_fonts);
        assert!(!opts.apply_background);

        transform_docx_with_options(&source, &template, &output, opts).unwrap();
        let (document, parts, _) = read_out(&output);

        assert!(document.contains("Acme Works Ltd"));
        assert!(
            !document.contains("behindDoc=\"1\""),
            "background chrome must stay off when background traits unselected"
        );
        assert!(
            !document.contains("<w:background"),
            "page fill must stay off when background traits unselected"
        );
        // Colours stripped from source; fonts kept.
        assert!(!document.contains("<w:color"));
        assert!(
            document.contains("Times New Roman") || document.contains("w:rFonts"),
            "source fonts should survive when font traits unselected"
        );

        let styles = parts
            .get("word/styles2.xml")
            .expect("styles2.xml from master");
        assert!(
            !styles.contains("w:rFonts"),
            "template style fonts scrubbed when fonts unselected"
        );
        assert!(
            styles.contains("w:color") || styles.contains("068145"),
            "template style colours kept when colour traits selected"
        );
    }

    #[test]
    fn transform_full_master_injects_behind_doc_into_first_text_para() {
        let master = PathBuf::from(
            "/cursor/stores/bc-dcb61577-a4fb-44e3-9ea5-6fcaadf23034/internal/fixtures/master-template-example.docx",
        );
        if !master.exists() || master.metadata().map(|m| m.len()).unwrap_or(0) < 1_000_000 {
            return;
        }

        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-test-full-bg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.docx");
        let output = dir.join("out.docx");

        write_minimal_docx(
            &source,
            "Heading1",
            "Normal",
            &[
                ("title", "Couples Alignment Blueprint"),
                ("body", "Factual body line that must stay near the top."),
            ],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();

        transform_docx(&source, &master, &output).unwrap();
        let (document, _, _) = read_out(&output);

        assert!(document.contains("Couples Alignment Blueprint"));
        assert!(document.contains("behindDoc=\"1\""));
        assert!(
            !document.contains("<wp:inline"),
            "must not prepend template inline photos"
        );
        assert!(
            document.contains("<w:background"),
            "theme accent2 should become w:background fill"
        );

        let paras: Vec<&str> = {
            let mut v = Vec::new();
            let mut rest = document.as_str();
            while let Some(start) = find_next_open_tag(rest, "w:p") {
                let from = &rest[start..];
                let Some(end) = find_element_end(from, "w:p") else {
                    break;
                };
                v.push(&from[..end]);
                rest = &from[end..];
            }
            v
        };
        assert!(
            !paras.is_empty()
                && paras[0].contains("behindDoc=\"1\"")
                && paragraph_has_meaningful_text(paras[0]),
            "first paragraph must carry behind-doc chrome AND the first text (no spacer para)"
        );
        assert!(
            paras[0].contains("relativeFrom=\"page\""),
            "chrome must be page-pinned"
        );
        assert_eq!(
            paras.iter().filter(|p| p.contains("behindDoc=\"1\"")).count(),
            1
        );
    }
}
