//! Regenerate a PDF from extracted document text + style cues.

use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};

use docx_core::extract_docx_text;

use crate::inspect::PdfStyleCues;
use crate::{PdfError, Result};

/// Options for PDF export / fixture generation.
#[derive(Debug, Clone)]
pub struct PdfExportOptions {
    pub title: String,
    pub cues: PdfStyleCues,
    pub apply_fonts: bool,
    pub apply_colours: bool,
    pub apply_background: bool,
}

impl Default for PdfExportOptions {
    fn default() -> Self {
        Self {
            title: "Compliancy Checker export".into(),
            cues: PdfStyleCues::default(),
            apply_fonts: true,
            apply_colours: true,
            apply_background: true,
        }
    }
}

/// Write a PDF from `(kind, text)` blocks (`heading` | `body`).
pub fn write_pdf_from_blocks(
    path: &Path,
    blocks: &[(&str, &str)],
    opts: &PdfExportOptions,
) -> Result<()> {
    let bytes = build_pdf_bytes(blocks, opts)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Extract text from a `.docx` and write a regenerated PDF.
pub fn write_pdf_from_docx(source: &Path, output: &Path, opts: &PdfExportOptions) -> Result<()> {
    let doc = extract_docx_text(source)?;
    let heading_set: std::collections::BTreeSet<String> =
        doc.headings.iter().cloned().collect();
    let mut blocks: Vec<(String, String)> = Vec::new();
    for line in doc.plain.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let kind = if heading_set.contains(trimmed) {
            "heading"
        } else {
            "body"
        };
        blocks.push((kind.into(), trimmed.to_string()));
    }
    if blocks.is_empty() {
        blocks.push(("body".into(), "(empty document)".into()));
    }
    let refs: Vec<(&str, &str)> = blocks
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    write_pdf_from_blocks(output, &refs, opts)
}

fn build_pdf_bytes(blocks: &[(&str, &str)], opts: &PdfExportOptions) -> Result<Vec<u8>> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    let body_face = if opts.apply_fonts {
        map_to_type1(&opts.cues.body_font, false)
    } else {
        "Helvetica"
    };
    let heading_face = if opts.apply_fonts {
        map_to_type1(&opts.cues.heading_font, true)
    } else {
        "Helvetica-Bold"
    };

    let font_body_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => Object::Name(body_face.as_bytes().to_vec()),
    });
    let font_heading_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => Object::Name(heading_face.as_bytes().to_vec()),
    });

    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => font_body_id,
            "F2" => font_heading_id,
        },
    });

    let page_w = opts.cues.page_width_pt.max(200.0);
    let page_h = opts.cues.page_height_pt.max(200.0);
    let margin = 54.0_f32; // 0.75"
    let max_width = page_w - margin * 2.0;

    let body_rgb = if opts.apply_colours {
        hex_to_rgb(&opts.cues.body_colour)
    } else {
        (0.0, 0.0, 0.0)
    };
    let heading_rgb = if opts.apply_colours {
        hex_to_rgb(&opts.cues.heading_colour)
    } else {
        (0.0, 0.0, 0.0)
    };

    let mut operations = Vec::new();

    if opts.apply_background {
        if let Some(ref fill) = opts.cues.background_fill {
            let (r, g, b) = hex_to_rgb(fill);
            operations.push(Operation::new(
                "rg",
                vec![r.into(), g.into(), b.into()],
            ));
            operations.push(Operation::new(
                "re",
                vec![0.into(), 0.into(), page_w.into(), page_h.into()],
            ));
            operations.push(Operation::new("f", vec![]));
        }
    }

    let mut y = page_h - margin;
    let body_size = 11.0_f32;
    let heading_size = 16.0_f32;
    let body_leading = 14.0_f32;
    let heading_leading = 20.0_f32;

    for (kind, text) in blocks {
        let is_heading = kind.eq_ignore_ascii_case("heading")
            || kind.eq_ignore_ascii_case("title");
        let (font, size, leading, rgb) = if is_heading {
            ("F2", heading_size, heading_leading, heading_rgb)
        } else {
            ("F1", body_size, body_leading, body_rgb)
        };

        let lines = wrap_text(text, max_width, size);
        for line in lines {
            if y < margin + leading {
                // Single-page v1: stop rather than invent multi-page for now.
                break;
            }
            operations.push(Operation::new("BT", vec![]));
            operations.push(Operation::new(
                "Tf",
                vec![Object::Name(font.as_bytes().to_vec()), size.into()],
            ));
            operations.push(Operation::new(
                "rg",
                vec![rgb.0.into(), rgb.1.into(), rgb.2.into()],
            ));
            operations.push(Operation::new(
                "Td",
                vec![margin.into(), y.into()],
            ));
            operations.push(Operation::new(
                "Tj",
                vec![Object::string_literal(pdf_safe(&line))],
            ));
            operations.push(Operation::new("ET", vec![]));
            y -= leading;
        }
        y -= if is_heading { 6.0 } else { 4.0 };
    }

    let content = Content { operations };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().map_err(|e| PdfError::Pdf(e.to_string()))?));

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), page_w.into(), page_h.into()],
        "Contents" => content_id,
        "Resources" => resources_id,
    });

    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => vec![page_id.into()],
        "Count" => 1,
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    // Optional info dict
    let info_id = doc.add_object(dictionary! {
        "Title" => Object::string_literal(opts.title.as_str()),
        "Producer" => Object::string_literal("Compliancy Checker"),
    });
    doc.trailer.set("Info", info_id);

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| PdfError::Pdf(e.to_string()))?;
    Ok(out)
}

fn map_to_type1(name: &str, bold: bool) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("times") || lower.contains("serif") || lower.contains("georgia") {
        if bold {
            "Times-Bold"
        } else {
            "Times-Roman"
        }
    } else if lower.contains("courier") || lower.contains("mono") {
        if bold {
            "Courier-Bold"
        } else {
            "Courier"
        }
    } else if bold || lower.contains("bold") {
        "Helvetica-Bold"
    } else {
        "Helvetica"
    }
}

fn hex_to_rgb(hex: &str) -> (f32, f32, f32) {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 {
        return (0.0, 0.0, 0.0);
    }
    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0) as f32 / 255.0;
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0) as f32 / 255.0;
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0) as f32 / 255.0;
    (r, g, b)
}

fn wrap_text(text: &str, max_width: f32, font_size: f32) -> Vec<String> {
    // Approximate average glyph width ~ 0.5 * size for Helvetica.
    let max_chars = ((max_width / (font_size * 0.5)).floor() as usize).max(20);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn pdf_safe(s: &str) -> String {
    // lopdf string_literal handles basic escaping; strip non-Latin1 for Type1 fonts.
    s.chars()
        .map(|c| if c.is_ascii() { c } else { '?' })
        .collect()
}
