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

pub use context::{
    apply_context_to_docx, evaluate_context_docx, evaluate_context_text, parse_context_rules,
    write_context_exports, ContextReport, ContextRule, RuleAction, RuleFinding, RuleOp,
};
pub use extract::{extract_docx_text, extract_from_document_xml, DocText};
pub use iso::{
    clauses_for, evaluate_docx, evaluate_text, merge_reports, parse_standards, write_exports,
    ChecklistClause, ChecklistReport, Finding, IsoStandard, Presence,
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
pub fn transform_docx(source: &Path, template: &Path, output: &Path) -> Result<()> {
    let source_bytes = std::fs::read(source)?;
    let template_bytes = std::fs::read(template)?;

    let mut source_zip = ZipArchive::new(Cursor::new(source_bytes))?;
    let mut template_zip = ZipArchive::new(Cursor::new(template_bytes))?;

    let source_document = read_zip_entry(&mut source_zip, DOCUMENT_PATH)?;
    let template_document = read_zip_entry(&mut template_zip, DOCUMENT_PATH)?;

    let source_inner = extract_body_inner(&source_document)?;
    let source_inner = strip_presentation_overrides(&source_inner);
    let merged_document = splice_body_content(&template_document, &source_inner)?;

    let mut parts: HashMap<String, Vec<u8>> = HashMap::new();
    for i in 0..template_zip.len() {
        let mut file = template_zip.by_index(i)?;
        let name = file.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        parts.insert(name, data);
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

/// Replace template body content (except trailing sectPr + chrome) with `source_inner`.
///
/// Preserves template `w:background` (outside `w:body`) via the document prefix, and
/// re-injects **leading behind-doc** body chrome so page fills stay behind text without
/// pulling in template inline photos (which would sit in-flow and push content down).
fn splice_body_content(template_document: &str, source_inner: &str) -> Result<String> {
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

    let chrome = extract_template_chrome(&inner_wo_sect);

    let mut new_inner = String::new();
    new_inner.push('\n');
    if !chrome.is_empty() {
        new_inner.push_str(&chrome);
        new_inner.push('\n');
    }
    new_inner.push_str(source_inner);
    new_inner.push('\n');
    if !sect_pr.is_empty() {
        new_inner.push_str(&sect_pr);
        new_inner.push('\n');
    }

    let mut out = String::with_capacity(template_document.len() + source_inner.len() + chrome.len());
    out.push_str(&template_document[..body_open]);
    out.push_str(&new_inner);
    out.push_str(&template_document[body_close..]);
    Ok(out)
}

/// Leading page-background chrome from the template body.
///
/// Word stores branded page fills as `wp:anchor behindDoc="1"` (DrawingML) and/or VML
/// picts with negative `z-index` — often in the first body paragraph(s), **before** real
/// text. Mid-document inline photos (`wp:inline`) are template *content*, not chrome:
/// prepending them puts images in front of / above the merged body and shifts text down.
///
/// We only keep behind-doc anchors from the **prefix** (until the first meaningful text
/// paragraph), and normalize paragraph spacing so chrome does not add vertical gap.
fn extract_template_chrome(template_body_inner: &str) -> String {
    let mut chrome = String::new();
    let mut rest = template_body_inner;
    while let Some(start) = find_next_open_tag(rest, "w:p") {
        let from = &rest[start..];
        let Some(end) = find_element_end(from, "w:p") else {
            break;
        };
        let para = &from[..end];
        if paragraph_has_meaningful_text(para) {
            // End of leading chrome — do not pull mid-doc section backgrounds or photos.
            break;
        }
        if is_behind_doc_chrome(para) {
            chrome.push_str(&normalize_chrome_paragraph(para));
            chrome.push('\n');
        }
        rest = &from[end..];
    }
    chrome
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

/// Zero out paragraph spacing on chrome so behind-doc anchors do not displace body text.
fn normalize_chrome_paragraph(para: &str) -> String {
    let spacing =
        r#"<w:spacing w:before="0" w:after="0" w:line="0" w:lineRule="auto"/>"#;
    if let Some(ppr_start) = find_next_open_tag(para, "w:pPr") {
        let from = &para[ppr_start..];
        if let Some(ppr_end) = find_element_end(from, "w:pPr") {
            let ppr = &from[..ppr_end];
            let first_gt = match ppr.find('>') {
                Some(i) => i,
                None => return para.to_string(),
            };
            let mut inner = if ppr.as_bytes().get(first_gt.saturating_sub(1)) == Some(&b'/') {
                String::new()
            } else {
                ppr[first_gt + 1..ppr.len() - "</w:pPr>".len()].to_string()
            };
            // Drop existing spacing children, then force zero spacing.
            inner = strip_named_children(&inner, &["w:spacing"]);
            let new_ppr = format!("<w:pPr>{spacing}{inner}</w:pPr>");
            return format!(
                "{}{}{}",
                &para[..ppr_start],
                new_ppr,
                &from[ppr_end..]
            );
        }
    }
    // No pPr — insert one after the opening <w:p …>.
    if let Some(gt) = para.find('>') {
        return format!(
            "{}<w:pPr>{spacing}</w:pPr>{}",
            &para[..=gt],
            &para[gt + 1..]
        );
    }
    para.to_string()
}

fn strip_named_children(inner: &str, names: &[&str]) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let from = &rest[lt..];
        if from.starts_with("</") {
            out.push('<');
            rest = &from[1..];
            continue;
        }
        let name_end = from[1..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .map(|i| i + 1)
            .unwrap_or(1);
        let name = &from[1..name_end];
        if names.iter().any(|s| *s == name) {
            if let Some(end) = find_element_end(from, name) {
                rest = &from[end..];
                continue;
            }
        }
        out.push('<');
        rest = &from[1..];
    }
    out.push_str(rest);
    out
}

/// Remove presentation direct-formatting that would override template style colours/fonts.
///
/// Keeps `w:pStyle` / `w:rStyle` so Heading1 vs Normal (and character styles) still select
/// distinct template colours. Semantic emphasis (`w:b`, `w:i`, …) is left intact.
fn strip_presentation_overrides(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<w:rPr") {
        out.push_str(&rest[..start]);
        let from = &rest[start..];
        let Some(end) = find_element_end(from, "w:rPr") else {
            out.push_str(from);
            return out;
        };
        let rpr = &from[..end];
        let cleaned = clean_rpr_element(rpr);
        out.push_str(&cleaned);
        rest = &from[end..];
    }
    out.push_str(rest);
    out
}

fn clean_rpr_element(rpr: &str) -> String {
    let first_gt = match rpr.find('>') {
        Some(i) => i,
        None => return rpr.to_string(),
    };
    // Self-closing empty rPr
    if rpr.as_bytes().get(first_gt.saturating_sub(1)) == Some(&b'/') {
        return String::new();
    }
    let inner = &rpr[first_gt + 1..rpr.len() - "</w:rPr>".len()];
    let cleaned_inner = strip_presentation_children(inner);
    if cleaned_inner
        .chars()
        .all(|c| c.is_whitespace())
    {
        return String::new();
    }
    format!("<w:rPr>{cleaned_inner}</w:rPr>")
}

fn strip_presentation_children(inner: &str) -> String {
    // Child local names that paint presentation from the source and must yield to template styles.
    const STRIP: &[&str] = &[
        "w:color",
        "w:rFonts",
        "w:sz",
        "w:szCs",
        "w:highlight",
        "w:shd",
        "w:bdr",
        "w:em",
        "w:snapToGrid",
        "w:spacing", // character spacing
        "w:w",       // character scale
        "w:kern",
        "w:position",
        "w:vanish",
    ];
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let from = &rest[lt..];
        if from.starts_with("</") {
            // Unexpected close — copy through
            out.push('<');
            rest = &from[1..];
            continue;
        }
        let name_end = from[1..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .map(|i| i + 1)
            .unwrap_or(1);
        let name = &from[1..name_end];
        if STRIP.iter().any(|s| *s == name) {
            if let Some(end) = find_element_end(from, name) {
                rest = &from[end..];
                continue;
            }
        }
        out.push('<');
        rest = &from[1..];
    }
    out.push_str(rest);
    out
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
            !document.contains("w:color"),
            "source direct w:color should be stripped; colours come from template styles"
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
        // Slim fixture chrome is behindDoc-only (no template inline photos prepended).
        assert!(
            !document.contains("<wp:inline"),
            "template inline photos must not be prepended as chrome (pushes text / sits in front)"
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
        let out = strip_presentation_overrides(xml);
        assert!(out.contains("w:pStyle w:val=\"Heading1\""));
        assert!(out.contains("<w:b/>"));
        assert!(out.contains("<w:i/>"));
        assert!(!out.contains("w:color"));
        assert!(!out.contains("w:rFonts"));
        assert!(!out.contains("w:sz"));
        assert!(out.contains(">Hi<"));
    }

    #[test]
    fn chrome_keeps_leading_behind_doc_skips_inline_and_mid_doc() {
        let body = r#"
<w:p><w:pPr><w:spacing w:before="240"/></w:pPr><w:r><w:drawing><wp:anchor behindDoc="1" relativeHeight="1"><wp:wrapNone/></wp:anchor></w:drawing></w:r></w:p>
<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1" cy="1"/></wp:inline></w:drawing></w:r></w:p>
<w:p><w:r><w:t>Who we are</w:t></w:r></w:p>
<w:p><w:r><w:drawing><wp:anchor behindDoc="1" relativeHeight="2"><wp:wrapNone/></wp:anchor></w:drawing></w:r></w:p>
"#;
        let chrome = extract_template_chrome(body);
        assert!(chrome.contains("behindDoc=\"1\""));
        assert!(chrome.contains("relativeHeight=\"1\""));
        assert!(
            !chrome.contains("<wp:inline"),
            "inline photos are not chrome"
        );
        assert!(
            !chrome.contains("relativeHeight=\"2\""),
            "mid-doc behindDoc after text must not be prepended"
        );
        assert!(
            chrome.contains(r#"w:spacing w:before="0""#),
            "chrome spacing normalized to avoid pushing text down"
        );
        assert!(!chrome.contains("Who we are"));
    }

    #[test]
    fn transform_full_master_does_not_prepend_inline_photos() {
        let master = PathBuf::from(
            "/cursor/stores/bc-dcb61577-a4fb-44e3-9ea5-6fcaadf23034/internal/fixtures/master-template-example.docx",
        );
        if !master.exists() || master.metadata().map(|m| m.len()).unwrap_or(0) < 1_000_000 {
            // Store copy of Beau's full master not available in this environment.
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
        // First text should appear early — not after a stack of photo paragraphs.
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
        let first_text = paras.iter().position(|p| paragraph_has_meaningful_text(p));
        assert!(
            matches!(first_text, Some(i) if i <= 2),
            "text should follow leading behind-doc chrome only, got first_text={first_text:?}"
        );
        let behind_before_text = paras
            .iter()
            .take(first_text.unwrap_or(0))
            .filter(|p| p.contains("behindDoc=\"1\""))
            .count();
        assert_eq!(
            behind_before_text, 1,
            "only the leading page background behindDoc should be prepended"
        );
    }
}
