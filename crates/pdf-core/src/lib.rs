//! PDF master-template inspection and PDF export for Compliancy Checker.
//!
//! ## What works (v1)
//! - Inspect a PDF master for **usable style/layout cues**: BaseFont names,
//!   page `MediaBox` size, and best-effort RGB samples from content streams.
//! - Present those cues in the same trait-picker shape as `.docx` masters.
//! - When a PDF is the master and output is `.docx`, synthesise a styles-only
//!   Word template from the cues and run the existing Open XML merge.
//! - Export/save as PDF by **regenerating** a new PDF from extracted document
//!   text + selected style cues (fonts mapped to standard Type1 faces).
//!
//! ## Limits (documented for operators)
//! - Not a binary restyle of an existing PDF; output is newly generated.
//! - Embedded custom fonts are **not** subset-copied; names map to Helvetica /
//!   Times / Courier families.
//! - Layout chrome (headers/footers, images, multi-column, absolute frames)
//!   is not reproduced.
//! - Colours are sampled from content operators when present — not a full
//!   theme/brand pack extraction.
//! - Scanned / image-only PDFs yield little or no usable cues (no OCR in v1).
//! - Margins are approximated from page size; crop boxes / bleed ignored.
//! - Tables and inline images from source `.docx` are flattened to plain text
//!   in PDF export.

mod export;
mod inspect;

pub use export::{write_pdf_from_blocks, write_pdf_from_docx, PdfExportOptions};
pub use inspect::{
    inspect_pdf_template, pdf_limits, cues_from_preview, PdfStyleCues, PDF_LIMITATIONS,
};

use std::path::Path;

use thiserror::Error;

use docx_core::{
    write_synthetic_style_template, SyntheticStyleCues, TemplatePreview, TraitSelection,
    TransformOptions,
};

#[derive(Debug, Error)]
pub enum PdfError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("pdf error: {0}")]
    Pdf(String),
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Docx(#[from] docx_core::DocxError),
}

pub type Result<T> = std::result::Result<T, PdfError>;

/// Inspect either a `.docx` or `.pdf` master template.
pub fn inspect_any_template(path: &Path) -> Result<TemplatePreview> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Ok(inspect_pdf_template(path)?),
        "docx" => Ok(docx_core::inspect_template(path)?),
        other => Err(PdfError::Message(format!(
            "unsupported template format '{other}' (use .docx or .pdf)"
        ))),
    }
}

/// Resolve transform options for any supported master format.
pub fn options_from_any_template(
    template: &Path,
    selection: &TraitSelection,
) -> Result<(TemplatePreview, TransformOptions)> {
    let preview = inspect_any_template(template)?;
    let opts = TransformOptions::from_selection(&preview.traits, selection);
    Ok((preview, opts))
}

/// Materialise a PDF master into a temporary `.docx` styles template for merge.
///
/// Returns `None` when `template` is already a `.docx` (caller uses it directly).
pub fn materialise_docx_master(
    template: &Path,
    selection: &TraitSelection,
    dest: &Path,
) -> Result<Option<(TemplatePreview, TransformOptions)>> {
    let ext = template
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "docx" {
        return Ok(None);
    }
    if ext != "pdf" {
        return Err(PdfError::Message(format!(
            "unsupported template format '{ext}'"
        )));
    }

    let preview = inspect_pdf_template(template)?;
    let opts = TransformOptions::from_selection(&preview.traits, selection);
    let cues = cues_from_preview(&preview, &opts);
    let synthetic = SyntheticStyleCues {
        body_font: cues.body_font.clone(),
        heading_font: cues.heading_font.clone(),
        body_colour: cues.body_colour.clone(),
        heading_colour: cues.heading_colour.clone(),
        page_width_twips: cues.page_width_twips(),
        page_height_twips: cues.page_height_twips(),
        background_fill: if opts.apply_background {
            cues.background_fill.clone()
        } else {
            None
        },
    };
    write_synthetic_style_template(dest, &synthetic)?;
    Ok(Some((preview, opts)))
}

/// True when path looks like a PDF.
pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// True when path looks like a Word Open XML package.
pub fn is_docx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("docx"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_core::{write_minimal_docx, SAMPLE_STYLES};
    use std::path::PathBuf;

    #[test]
    fn roundtrip_fixture_pdf_inspect_and_export() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let master = dir.join("master-template.pdf");
        write_pdf_from_blocks(
            &master,
            &[
                ("heading", "Master PDF Template"),
                ("body", "Body sample with brand colour."),
            ],
            &PdfExportOptions {
                title: "Master".into(),
                cues: PdfStyleCues {
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

        let preview = inspect_pdf_template(&master).unwrap();
        assert_eq!(preview.format, "pdf");
        assert!(!preview.traits.is_empty());
        assert!(!preview.limits.is_empty());
        assert!(preview.summary.fonts.iter().any(|f| f.contains("Helvetica")));

        let source = dir.join("source.docx");
        write_minimal_docx(
            &source,
            "Heading1",
            "Normal",
            &[
                ("title", "Site Safety Report"),
                ("body", "Customer: Acme Works Ltd"),
            ],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();

        let out_pdf = dir.join("out.pdf");
        write_pdf_from_docx(
            &source,
            &out_pdf,
            &PdfExportOptions {
                title: "Export".into(),
                cues: cues_from_preview(&preview, &TransformOptions::all()),
                apply_fonts: true,
                apply_colours: true,
                apply_background: true,
            },
        )
        .unwrap();
        assert!(out_pdf.metadata().unwrap().len() > 200);

        let synthetic = dir.join("synthetic.docx");
        let result = materialise_docx_master(
            &master,
            &TraitSelection {
                selected_ids: None,
            },
            &synthetic,
        )
        .unwrap();
        assert!(result.is_some());
        assert!(synthetic.exists());
        let syn_preview = docx_core::inspect_template(&synthetic).unwrap();
        assert!(!syn_preview.traits.is_empty());
    }
}
