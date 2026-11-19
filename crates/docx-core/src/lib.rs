//! Spike: merge factual content from a source `.docx` into a master template package.
//!
//! Strategy (Open XML):
//! 1. Start from the **template** package (styles, theme, fonts, headers/footers, structure).
//! 2. Extract body block-level children from the **source** `word/document.xml`
//!    (everything inside `<w:body>` except a trailing `<w:sectPr>`).
//! 3. Keep the template's trailing `<w:sectPr>` (page setup / section properties).
//! 4. Write a new package under `output`.
//!
//! Style IDs on source paragraphs (e.g. `Heading1`, `Normal`) resolve against the
//! template's `styles.xml`, so fonts/colours come from the master as far as Open XML allows.

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

const DOCUMENT_PATH: &str = "word/document.xml";

/// Transform `source` using styles/structure from `template`, writing `output`.
pub fn transform_docx(source: &Path, template: &Path, output: &Path) -> Result<()> {
    let source_bytes = std::fs::read(source)?;
    let template_bytes = std::fs::read(template)?;

    let mut source_zip = ZipArchive::new(Cursor::new(source_bytes))?;
    let mut template_zip = ZipArchive::new(Cursor::new(template_bytes))?;

    let source_document = read_zip_entry(&mut source_zip, DOCUMENT_PATH)?;
    let template_document = read_zip_entry(&mut template_zip, DOCUMENT_PATH)?;

    let source_inner = extract_body_inner(&source_document)?;
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

fn read_zip_entry<R: Read + std::io::Seek>(
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
fn splice_body_content(template_document: &str, source_inner: &str) -> Result<String> {
    let body_open = find_tag_end(template_document, "w:body")
        .ok_or_else(|| DocxError::Message("template missing <w:body>".into()))?;
    let body_close = template_document
        .rfind("</w:body>")
        .ok_or_else(|| DocxError::Message("template missing </w:body>".into()))?;

    let template_inner = &template_document[body_open..body_close];
    let mut sect_pr = String::new();
    if let Some(sect_start) = find_last_open_tag(template_inner, "w:sectPr") {
        if let Some(sect_end_rel) = find_element_end(&template_inner[sect_start..], "w:sectPr") {
            sect_pr = template_inner[sect_start..sect_start + sect_end_rel].to_string();
        }
    }

    let mut new_inner = String::new();
    new_inner.push('\n');
    new_inner.push_str(source_inner);
    new_inner.push('\n');
    if !sect_pr.is_empty() {
        new_inner.push_str(&sect_pr);
        new_inner.push('\n');
    }

    let mut out = String::with_capacity(template_document.len() + source_inner.len());
    out.push_str(&template_document[..body_open]);
    out.push_str(&new_inner);
    out.push_str(&template_document[body_close..]);
    Ok(out)
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
        ("word/document.xml", document_xml.as_str()),
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

fn xml_escape(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

        let out_bytes = std::fs::read(&output).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(out_bytes)).unwrap();
        let document = read_zip_entry(&mut zip, DOCUMENT_PATH).unwrap();
        let styles = read_zip_entry(&mut zip, "word/styles.xml").unwrap();

        assert!(document.contains("Acme Works Ltd"));
        assert!(document.contains("12 Harbour Road, Bristol BS1 1AA"));
        assert!(!document.contains("TEMPLATE PLACEHOLDER"));
        assert!(styles.contains("Calibri"));
        assert!(styles.contains("C45911"));
        assert!(!styles.contains("Times New Roman"));
    }
}
