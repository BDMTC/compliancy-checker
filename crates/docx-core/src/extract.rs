//! Extract plain text and heading candidates from a `.docx` package.

use std::io::Cursor;
use std::path::Path;

use zip::ZipArchive;

use crate::{read_zip_entry, Result, DOCUMENT_PATH};

/// Plain-text snapshot of a document for checklist matching.
#[derive(Debug, Clone)]
pub struct DocText {
    /// Concatenated paragraph / run text (whitespace-normalized).
    pub plain: String,
    /// Texts from paragraphs that look like headings (style or outline).
    pub headings: Vec<String>,
}

impl DocText {
    /// Lowercased haystack used for case-insensitive presence checks.
    pub fn search_blob(&self) -> String {
        let mut blob = self.plain.to_lowercase();
        blob.push('\n');
        for h in &self.headings {
            blob.push_str(&h.to_lowercase());
            blob.push('\n');
        }
        blob
    }
}

/// Read `word/document.xml` from `path` and extract text + heading candidates.
pub fn extract_docx_text(path: &Path) -> Result<DocText> {
    let bytes = std::fs::read(path)?;
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let document = read_zip_entry(&mut zip, DOCUMENT_PATH)?;
    Ok(extract_from_document_xml(&document))
}

/// Parse body text from a `word/document.xml` string.
pub fn extract_from_document_xml(document_xml: &str) -> DocText {
    let mut plain_parts: Vec<String> = Vec::new();
    let mut headings: Vec<String> = Vec::new();

    for para in iter_paragraphs(document_xml) {
        let text = collect_run_text(para);
        if text.trim().is_empty() {
            continue;
        }
        let normalized = collapse_ws(&text);
        plain_parts.push(normalized.clone());
        if paragraph_looks_like_heading(para) {
            headings.push(normalized);
        }
    }

    DocText {
        plain: plain_parts.join("\n"),
        headings,
    }
}

fn iter_paragraphs(xml: &str) -> impl Iterator<Item = &str> {
    ParagraphIter { xml, pos: 0 }
}

struct ParagraphIter<'a> {
    xml: &'a str,
    pos: usize,
}

impl<'a> Iterator for ParagraphIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = &self.xml[self.pos..];
        let start_rel = rest.find("<w:p")?;
        // Avoid matching <w:pPr>, <w:pStyle>, etc. — require end of tag name.
        let absolute = self.pos + start_rel;
        let after_name = absolute + "<w:p".len();
        let next = self.xml.as_bytes().get(after_name).copied();
        if next != Some(b'>') && next != Some(b' ') && next != Some(b'/') && next != Some(b'\n')
            && next != Some(b'\r') && next != Some(b'\t')
        {
            self.pos = after_name;
            return self.next();
        }

        let from_open = &self.xml[absolute..];
        let end_rel = find_paragraph_end(from_open)?;
        let slice = &self.xml[absolute..absolute + end_rel];
        self.pos = absolute + end_rel;
        Some(slice)
    }
}

fn find_paragraph_end(xml: &str) -> Option<usize> {
    let first_gt = xml.find('>')?;
    if xml[..first_gt].ends_with('/')
        || xml.as_bytes().get(first_gt.saturating_sub(1)) == Some(&b'/')
    {
        return Some(first_gt + 1);
    }
    // Nested <w:p> is rare in body; match first closing tag.
    let close = "</w:p>";
    let close_at = xml.find(close)?;
    Some(close_at + close.len())
}

fn paragraph_looks_like_heading(para_xml: &str) -> bool {
    let lower = para_xml.to_lowercase();
    if lower.contains("w:pstyle") {
        // Common Word heading style ids / names
        if lower.contains("heading")
            || lower.contains("w:val=\"title\"")
            || lower.contains("w:val='title'")
        {
            return true;
        }
    }
    // Outline level 0–2 ≈ heading-ish
    if let Some(idx) = lower.find("w:outlinelvl") {
        let slice = &lower[idx..idx.saturating_add(40)];
        if slice.contains("w:val=\"0\"")
            || slice.contains("w:val=\"1\"")
            || slice.contains("w:val=\"2\"")
            || slice.contains("w:val='0'")
            || slice.contains("w:val='1'")
            || slice.contains("w:val='2'")
        {
            return true;
        }
    }
    false
}

fn collect_run_text(para_xml: &str) -> String {
    let mut out = String::new();
    let mut rest = para_xml;
    while let Some(t_start) = find_t_open(rest) {
        let after_open = &rest[t_start..];
        let content_start = match after_open.find('>') {
            Some(i) => i + 1,
            None => break,
        };
        let content = &after_open[content_start..];
        let end = match content.find("</w:t>") {
            Some(i) => i,
            None => break,
        };
        out.push_str(&xml_unescape(&content[..end]));
        rest = &content[end + "</w:t>".len()..];
    }
    out
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

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_and_heading_from_minimal_xml() {
        let xml = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:t>4.1 Context of the organization</w:t></w:r>
    </w:p>
    <w:p>
      <w:r><w:t>We reviewed interested parties.</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;
        let doc = extract_from_document_xml(xml);
        assert!(doc.plain.contains("4.1 Context of the organization"));
        assert!(doc.plain.contains("interested parties"));
        assert_eq!(doc.headings.len(), 1);
        assert!(doc.headings[0].contains("4.1"));
    }
}
