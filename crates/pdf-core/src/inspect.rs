//! Best-effort PDF master-template inspection.

use std::collections::BTreeSet;
use std::path::Path;

use lopdf::{Dictionary, Document, Object};

use docx_core::{DetectedTrait, TemplatePreview, TemplatePreviewSummary, TraitKind, TransformOptions};

use crate::{PdfError, Result};

/// Human-readable limits shown in the UI / README for PDF masters.
pub const PDF_LIMITATIONS: &[&str] = &[
    "PDF masters supply style/layout cues only — not a pixel-perfect restyle.",
    "Fonts map by BaseFont name to Helvetica / Times / Courier families; embedded faces are not copied.",
    "Colours are sampled from content stream operators when present (best-effort).",
    "Page size comes from MediaBox; headers/footers/images/multi-column layout are not applied.",
    "Scanned or image-only PDFs yield little or no usable cues (OCR is out of scope for v1).",
    "When exporting Word, cues become a synthetic styles template for the existing Open XML merge.",
];

/// Resolved style cues used for PDF export and synthetic `.docx` masters.
#[derive(Debug, Clone)]
pub struct PdfStyleCues {
    pub body_font: String,
    pub heading_font: String,
    pub body_colour: String,
    pub heading_colour: String,
    /// Page width in PDF points (1/72").
    pub page_width_pt: f32,
    /// Page height in PDF points.
    pub page_height_pt: f32,
    pub background_fill: Option<String>,
}

impl Default for PdfStyleCues {
    fn default() -> Self {
        Self {
            body_font: "Helvetica".into(),
            heading_font: "Helvetica-Bold".into(),
            body_colour: "000000".into(),
            heading_colour: "000000".into(),
            page_width_pt: 612.0,
            page_height_pt: 792.0,
            background_fill: None,
        }
    }
}

impl PdfStyleCues {
    pub fn page_width_twips(&self) -> u32 {
        (self.page_width_pt * 20.0).round() as u32
    }

    pub fn page_height_twips(&self) -> u32 {
        (self.page_height_pt * 20.0).round() as u32
    }
}

/// Static limits list for UI copy.
pub fn pdf_limits() -> Vec<String> {
    PDF_LIMITATIONS.iter().map(|s| (*s).to_string()).collect()
}

/// Inspect a PDF and return selectable traits in the shared preview shape.
pub fn inspect_pdf_template(path: &Path) -> Result<TemplatePreview> {
    let doc = Document::load(path).map_err(|e| PdfError::Pdf(e.to_string()))?;

    let mut fonts: BTreeSet<String> = BTreeSet::new();
    let mut colours: BTreeSet<String> = BTreeSet::new();
    let mut page_w = 612.0_f32;
    let mut page_h = 792.0_f32;
    let mut pages_seen = 0usize;

    let pages = doc.get_pages();
    for (_num, page_id) in pages {
        pages_seen += 1;
        if let Ok(dict) = doc.get_dictionary(page_id) {
            if let Some((w, h)) = media_box_size(&doc, dict) {
                if pages_seen == 1 {
                    page_w = w;
                    page_h = h;
                }
            }
            collect_fonts_from_resources(&doc, dict, &mut fonts);
            if let Ok(content) = doc.get_page_content(page_id) {
                sample_colours_from_content(&content, &mut colours);
            }
        }
    }

    // Also walk the whole document for Font dictionaries (covers shared resources).
    for object in doc.objects.values() {
        if let Object::Dictionary(dict) = object {
            if dict.get(b"Type").ok().and_then(as_name).as_deref() == Some("Font") {
                if let Some(base) = dict
                    .get(b"BaseFont")
                    .ok()
                    .and_then(as_name)
                    .or_else(|| dict.get(b"BaseFont").ok().and_then(as_string))
                {
                    let cleaned = clean_base_font(&base);
                    if !cleaned.is_empty() {
                        fonts.insert(cleaned);
                    }
                }
            }
        }
    }

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("template.pdf")
        .to_string();

    let mut traits = Vec::new();

    traits.push(DetectedTrait {
        id: "layout:page-size".into(),
        kind: TraitKind::Background,
        label: "Page size".into(),
        value: format!("{page_w:.0}×{page_h:.0} pt"),
        style_id: None,
        swatch: None,
    });

    for font in &fonts {
        traits.push(DetectedTrait {
            id: format!("font:pdf:{font}"),
            kind: TraitKind::Font,
            label: "Font".into(),
            value: font.clone(),
            style_id: None,
            swatch: None,
        });
    }

    for colour in &colours {
        traits.push(DetectedTrait {
            id: format!("colour:pdf:{colour}"),
            kind: TraitKind::Colour,
            label: "Colour".into(),
            value: format!("#{colour}"),
            style_id: None,
            swatch: Some(colour.clone()),
        });
    }

    // Prefer a light sample as background candidate when present.
    let background_fill = colours
        .iter()
        .find(|c| is_light_fill(c))
        .cloned()
        .or_else(|| colours.iter().next().cloned());

    if let Some(ref fill) = background_fill {
        traits.push(DetectedTrait {
            id: "background:pdf:fill".into(),
            kind: TraitKind::Background,
            label: "Background fill (sampled)".into(),
            value: format!("#{fill}"),
            style_id: None,
            swatch: Some(fill.clone()),
        });
    }

    if traits.len() == 1 {
        // Only page size — still useful, but warn via limits.
        traits.push(DetectedTrait {
            id: "font:pdf:Helvetica".into(),
            kind: TraitKind::Font,
            label: "Font (fallback)".into(),
            value: "Helvetica".into(),
            style_id: None,
            swatch: None,
        });
        fonts.insert("Helvetica".into());
    }

    Ok(TemplatePreview {
        path: path.display().to_string(),
        file_name,
        format: "pdf".into(),
        traits,
        summary: TemplatePreviewSummary {
            fonts: fonts.into_iter().collect(),
            colours: colours.into_iter().collect(),
            background_fill,
            has_background_chrome: false,
            style_count: pages_seen,
        },
        limits: pdf_limits(),
    })
}

/// Build export/synthetic cues from a PDF (or docx-shaped) preview + selection flags.
pub fn cues_from_preview(preview: &TemplatePreview, opts: &TransformOptions) -> PdfStyleCues {
    let mut cues = PdfStyleCues::default();

    if let Some(page) = preview
        .traits
        .iter()
        .find(|t| t.id == "layout:page-size")
    {
        if let Some((w, h)) = parse_page_size_label(&page.value) {
            cues.page_width_pt = w;
            cues.page_height_pt = h;
        }
    } else if preview.format == "docx" {
        // Letter default already set.
    }

    let fonts: Vec<&str> = preview
        .summary
        .fonts
        .iter()
        .map(|s| s.as_str())
        .collect();
    if opts.apply_fonts {
        if let Some(body) = fonts
            .iter()
            .copied()
            .find(|f| !f.to_ascii_lowercase().contains("bold") && !f.contains("Black"))
            .or_else(|| fonts.first().copied())
        {
            cues.body_font = map_to_word_or_pdf_font(body, false);
            cues.heading_font = map_to_word_or_pdf_font(
                fonts
                    .iter()
                    .copied()
                    .find(|f| f.to_ascii_lowercase().contains("bold"))
                    .unwrap_or(body),
                true,
            );
        }
    }

    let colours = &preview.summary.colours;
    if opts.apply_colours {
        if let Some(body) = colours
            .iter()
            .find(|c| !is_light_fill(c))
            .cloned()
            .or_else(|| colours.first().cloned())
        {
            cues.body_colour = body.clone();
            cues.heading_colour = colours
                .iter()
                .find(|c| **c != body && !is_light_fill(c))
                .cloned()
                .unwrap_or(body);
        }
    }

    if opts.apply_background {
        cues.background_fill = preview
            .summary
            .background_fill
            .clone()
            .or_else(|| colours.iter().find(|c| is_light_fill(c)).cloned());
    }

    // Twips helpers live on cues; keep PDF pt for export.
    let _ = (cues.page_width_twips(), cues.page_height_twips());
    cues
}

fn parse_page_size_label(value: &str) -> Option<(f32, f32)> {
    // "612×792 pt"
    let cleaned = value.replace('×', "x").replace(" pt", "");
    let mut parts = cleaned.split('x');
    let w: f32 = parts.next()?.trim().parse().ok()?;
    let h: f32 = parts.next()?.trim().parse().ok()?;
    Some((w, h))
}

/// Map a PDF BaseFont / Word face into a Word-friendly name (for synthetic docx)
/// or keep a PDF Type1-ish name for export mapping later.
pub fn map_to_word_or_pdf_font(name: &str, prefer_bold: bool) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.contains("times") || lower.contains("serif") || lower.contains("georgia") {
        if prefer_bold {
            "Times New Roman".into()
        } else {
            "Times New Roman".into()
        }
    } else if lower.contains("courier") || lower.contains("mono") || lower.contains("consolas") {
        "Courier New".into()
    } else if lower.contains("calibri") {
        "Calibri".into()
    } else if lower.contains("arial") {
        "Arial".into()
    } else if lower.contains("helvetica") || lower.contains("sans") {
        if prefer_bold {
            "Arial".into()
        } else {
            "Arial".into()
        }
    } else {
        // Preserve original name for Word; PDF export will remap to Type1.
        name.trim().to_string()
    }
}

fn media_box_size(doc: &Document, page: &Dictionary) -> Option<(f32, f32)> {
    let mb = page.get(b"MediaBox").ok()?;
    let arr = match mb {
        Object::Array(a) => a.clone(),
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Array(a) => a.clone(),
            _ => return None,
        },
        _ => return None,
    };
    if arr.len() < 4 {
        return None;
    }
    let x0 = as_f32(&arr[0])?;
    let y0 = as_f32(&arr[1])?;
    let x1 = as_f32(&arr[2])?;
    let y1 = as_f32(&arr[3])?;
    Some(((x1 - x0).abs(), (y1 - y0).abs()))
}

fn collect_fonts_from_resources(doc: &Document, page: &Dictionary, fonts: &mut BTreeSet<String>) {
    let Ok(res_obj) = page.get(b"Resources") else {
        return;
    };
    let res_dict = match resolve_dict(doc, res_obj) {
        Some(d) => d,
        None => return,
    };
    let Ok(font_obj) = res_dict.get(b"Font") else {
        return;
    };
    let Some(font_dict) = resolve_dict(doc, font_obj) else {
        return;
    };
    for (_name, obj) in font_dict.iter() {
        if let Some(dict) = resolve_dict(doc, obj) {
            if let Some(base) = dict
                .get(b"BaseFont")
                .ok()
                .and_then(as_name)
                .or_else(|| dict.get(b"BaseFont").ok().and_then(as_string))
            {
                let cleaned = clean_base_font(&base);
                if !cleaned.is_empty() {
                    fonts.insert(cleaned);
                }
            }
        }
    }
}

fn sample_colours_from_content(content: &[u8], colours: &mut BTreeSet<String>) {
    let text = String::from_utf8_lossy(content);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "rg" | "RG" if i >= 3 => {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    tokens[i - 3].parse::<f32>(),
                    tokens[i - 2].parse::<f32>(),
                    tokens[i - 1].parse::<f32>(),
                ) {
                    colours.insert(rgb_to_hex(r, g, b));
                }
                i += 1;
            }
            "g" | "G" if i >= 1 => {
                if let Ok(v) = tokens[i - 1].parse::<f32>() {
                    colours.insert(rgb_to_hex(v, v, v));
                }
                i += 1;
            }
            "k" | "K" if i >= 4 => {
                if let (Ok(c), Ok(m), Ok(y), Ok(k)) = (
                    tokens[i - 4].parse::<f32>(),
                    tokens[i - 3].parse::<f32>(),
                    tokens[i - 2].parse::<f32>(),
                    tokens[i - 1].parse::<f32>(),
                ) {
                    let r = 1.0 - (c + k).min(1.0);
                    let g = 1.0 - (m + k).min(1.0);
                    let b = 1.0 - (y + k).min(1.0);
                    colours.insert(rgb_to_hex(r, g, b));
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
}

fn rgb_to_hex(r: f32, g: f32, b: f32) -> String {
    let clamp = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("{:02X}{:02X}{:02X}", clamp(r), clamp(g), clamp(b))
}

fn is_light_fill(hex: &str) -> bool {
    if hex.len() != 6 {
        return false;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as u32;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as u32;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as u32;
    (r + g + b) > 600
}

fn clean_base_font(name: &str) -> String {
    // Strip subset prefix like "ABCDEF+Calibri"
    let s = name.rsplit('+').next().unwrap_or(name).trim();
    s.trim_start_matches('/').to_string()
}

fn resolve_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Dictionary(d) => Some(d),
            _ => None,
        },
        _ => None,
    }
}

fn as_name(obj: &Object) -> Option<String> {
    match obj {
        Object::Name(n) => Some(String::from_utf8_lossy(n).to_string()),
        _ => None,
    }
}

fn as_string(obj: &Object) -> Option<String> {
    match obj {
        Object::String(s, _) => Some(String::from_utf8_lossy(s).to_string()),
        _ => None,
    }
}

fn as_f32(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        Object::Reference(_) => None,
        _ => None,
    }
}
