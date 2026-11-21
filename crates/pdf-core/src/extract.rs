//! Best-effort plain-text extraction from PDF content streams (no OCR).

use std::path::Path;

use lopdf::Document;

use docx_core::DocText;

use crate::{PdfError, Result};

/// Honest limits for PDF text ingest / ISO RUN FIRST (logged to operators).
pub const PDF_TEXT_LIMITATIONS: &[&str] = &[
    "PDF text ingest is best-effort from content streams (Tj/TJ); not a full layout parse.",
    "Scanned or image-only PDFs yield little or no text — OCR is out of scope for v1.",
    "Some fonts/encodings may decode poorly; missing glyphs can drop words.",
    "ISO RUN FIRST on PDFs uses this extracted text only; clause presence may under-report.",
];

/// Static limits list for UI / run-log copy.
pub fn pdf_text_limits() -> Vec<String> {
    PDF_TEXT_LIMITATIONS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Extract searchable plain text from a PDF for ISO checklist / PDF→Word paths.
pub fn extract_pdf_text(path: &Path) -> Result<DocText> {
    let doc = Document::load(path).map_err(|e| PdfError::Pdf(e.to_string()))?;
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Ok(DocText {
            plain: String::new(),
            headings: Vec::new(),
        });
    }
    let mut page_nums: Vec<u32> = pages.keys().copied().collect();
    page_nums.sort_unstable();
    let raw = doc
        .extract_text(&page_nums)
        .map_err(|e| PdfError::Pdf(e.to_string()))?;
    let plain = normalize_extracted(&raw);
    Ok(DocText {
        plain,
        headings: Vec::new(),
    })
}

fn normalize_extracted(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            lines.push(collapsed);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{write_pdf_from_blocks, PdfExportOptions};
    use crate::inspect::PdfStyleCues;
    use std::path::PathBuf;

    #[test]
    fn extracts_text_from_generated_pdf() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/extract-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("sample.pdf");
        write_pdf_from_blocks(
            &pdf,
            &[
                ("heading", "4.1 Context of the organization"),
                ("body", "OH&S policy and worker consultation."),
            ],
            &PdfExportOptions {
                title: "Extract test".into(),
                cues: PdfStyleCues::default(),
                apply_fonts: true,
                apply_colours: true,
                apply_background: false,
            },
        )
        .unwrap();

        let text = extract_pdf_text(&pdf).unwrap();
        let blob = text.search_blob();
        assert!(
            blob.contains("context of the organization") || blob.contains("4.1"),
            "blob={blob:?}"
        );
        assert!(pdf_text_limits().len() >= 3);
    }
}
