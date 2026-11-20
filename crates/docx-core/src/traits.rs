//! Detect selectable master-template traits (fonts, colours, backgrounds) and
//! derive merge [`TransformOptions`] from a UI/CLI selection.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Cursor, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::{DocxError, Result, DOCUMENT_PATH};

/// Kind of a detected template trait shown in the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TraitKind {
    Font,
    Colour,
    Background,
}

impl TraitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TraitKind::Font => "font",
            TraitKind::Colour => "colour",
            TraitKind::Background => "background",
        }
    }
}

/// One selectable trait discovered in a master template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedTrait {
    /// Stable id, e.g. `font:style:Normal`, `colour:style:Heading1`, `background:fill`.
    pub id: String,
    pub kind: TraitKind,
    pub label: String,
    /// Human-readable value (font name, `#RRGGBB`, chrome summary).
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_id: Option<String>,
    /// Hex colour without `#` when this trait can show a swatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swatch: Option<String>,
}

/// Compact preview summary for the template panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePreviewSummary {
    pub fonts: Vec<String>,
    pub colours: Vec<String>,
    pub background_fill: Option<String>,
    pub has_background_chrome: bool,
    pub style_count: usize,
}

/// Full inspect result: traits + preview, ready for the UI picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePreview {
    pub path: String,
    pub file_name: String,
    /// `docx` or `pdf` — which inspect path produced this preview.
    #[serde(default = "default_template_format")]
    pub format: String,
    pub traits: Vec<DetectedTrait>,
    pub summary: TemplatePreviewSummary,
    /// Documented capability limits (especially for PDF masters).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
}

fn default_template_format() -> String {
    "docx".into()
}

/// Which detected traits the user wants applied on merge.
///
/// - `selected_ids: None` → apply **all** detected traits (default).
/// - `selected_ids: Some([])` → apply none.
/// - `selected_ids: Some([...])` → apply that subset (by trait id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TraitSelection {
    pub selected_ids: Option<Vec<String>>,
}

/// Merge behaviour derived from a trait selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformOptions {
    pub apply_fonts: bool,
    pub apply_colours: bool,
    pub apply_background: bool,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self::all()
    }
}

impl TransformOptions {
    pub fn all() -> Self {
        Self {
            apply_fonts: true,
            apply_colours: true,
            apply_background: true,
        }
    }

    pub fn none() -> Self {
        Self {
            apply_fonts: false,
            apply_colours: false,
            apply_background: false,
        }
    }

    /// Map a picker selection onto category flags used by the merge path.
    pub fn from_selection(traits: &[DetectedTrait], selection: &TraitSelection) -> Self {
        match &selection.selected_ids {
            None => Self::all(),
            Some(ids) if ids.is_empty() => Self::none(),
            Some(ids) => {
                let set: BTreeSet<&str> = ids.iter().map(|s| s.as_str()).collect();
                let mut opts = Self::none();
                for t in traits {
                    if !set.contains(t.id.as_str()) {
                        continue;
                    }
                    match t.kind {
                        TraitKind::Font => opts.apply_fonts = true,
                        TraitKind::Colour => opts.apply_colours = true,
                        TraitKind::Background => opts.apply_background = true,
                    }
                }
                opts
            }
        }
    }

    /// Parse a CLI list like `fonts,colours,background` or `all` / `none`.
    pub fn from_csv(csv: &str) -> Result<Self> {
        let raw = csv.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("all") {
            return Ok(Self::all());
        }
        if raw.eq_ignore_ascii_case("none") {
            return Ok(Self::none());
        }
        let mut opts = Self::none();
        for part in raw.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "" => {}
                "font" | "fonts" => opts.apply_fonts = true,
                "colour" | "colours" | "color" | "colors" => opts.apply_colours = true,
                "background" | "backgrounds" | "chrome" => opts.apply_background = true,
                other => {
                    return Err(DocxError::Message(format!(
                        "unknown trait category '{other}' (use fonts,colours,background)"
                    )));
                }
            }
        }
        Ok(opts)
    }
}

/// Inspect a master `.docx` and return selectable traits + preview summary.
pub fn inspect_template(path: &Path) -> Result<TemplatePreview> {
    let bytes = std::fs::read(path)?;
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;

    let mut parts: HashMap<String, String> = HashMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let name = file.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        if !(name.starts_with("word/styles")
            || name.starts_with("word/theme/")
            || name == DOCUMENT_PATH)
        {
            continue;
        }
        let mut buf = String::new();
        // Theme/styles/document are XML text; skip binary.
        if file.read_to_string(&mut buf).is_ok() {
            parts.insert(name, buf);
        }
    }

    let document = parts
        .get(DOCUMENT_PATH)
        .cloned()
        .ok_or_else(|| DocxError::Message(format!("missing zip entry: {DOCUMENT_PATH}")))?;

    let styles_xml = parts
        .iter()
        .find(|(k, _)| {
            let base = k.rsplit('/').next().unwrap_or(k.as_str());
            base.starts_with("styles") && base.ends_with(".xml")
        })
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    let theme_xml = parts
        .iter()
        .find(|(k, _)| k.starts_with("word/theme/") && k.ends_with(".xml"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    let mut traits = Vec::new();
    let mut fonts_seen: BTreeSet<String> = BTreeSet::new();
    let mut colours_seen: BTreeSet<String> = BTreeSet::new();
    let mut style_count = 0usize;

    // Per-style fonts + colours from styles*.xml
    for (style_id, block) in iter_style_blocks(styles_xml) {
        style_count += 1;
        if let Some(font) = first_attr_in(block, "w:rFonts", "w:ascii")
            .or_else(|| first_attr_in(block, "w:rFonts", "w:hAnsi"))
        {
            if fonts_seen.insert(font.clone()) {
                // also collect unique later; we still emit per-style traits
            }
            traits.push(DetectedTrait {
                id: format!("font:style:{style_id}"),
                kind: TraitKind::Font,
                label: format!("Font · {style_id}"),
                value: font.clone(),
                style_id: Some(style_id.clone()),
                swatch: None,
            });
            let _ = fonts_seen.insert(font);
        }
        if let Some(color) = first_attr_in(block, "w:color", "w:val") {
            let hex = normalize_hex(&color);
            if let Some(h) = hex {
                colours_seen.insert(h.clone());
                traits.push(DetectedTrait {
                    id: format!("colour:style:{style_id}"),
                    kind: TraitKind::Colour,
                    label: format!("Colour · {style_id}"),
                    value: format!("#{h}"),
                    style_id: Some(style_id.clone()),
                    swatch: Some(h),
                });
            }
        }
    }

    // Theme major/minor fonts
    for (slot, typeface) in theme_latin_fonts(theme_xml) {
        if fonts_seen.insert(typeface.clone()) || !traits.iter().any(|t| t.value == typeface) {
            traits.push(DetectedTrait {
                id: format!("font:theme:{slot}"),
                kind: TraitKind::Font,
                label: format!("Font · theme {slot}"),
                value: typeface.clone(),
                style_id: None,
                swatch: None,
            });
            fonts_seen.insert(typeface);
        }
    }

    // Theme colour scheme accents (preview + selectable)
    for (name, hex) in theme_scheme_colours(theme_xml) {
        colours_seen.insert(hex.clone());
        traits.push(DetectedTrait {
            id: format!("colour:theme:{name}"),
            kind: TraitKind::Colour,
            label: format!("Colour · theme {name}"),
            value: format!("#{hex}"),
            style_id: None,
            swatch: Some(hex),
        });
    }

    // Document background fill
    let background_fill = document_background_hex(&document).or_else(|| {
        theme_scheme_colours(theme_xml)
            .into_iter()
            .find(|(n, _)| n == "accent2")
            .map(|(_, h)| h)
    });
    if let Some(ref fill) = background_fill {
        colours_seen.insert(fill.clone());
        traits.push(DetectedTrait {
            id: "background:fill".into(),
            kind: TraitKind::Background,
            label: "Background · page fill".into(),
            value: format!("#{fill}"),
            style_id: None,
            swatch: Some(fill.clone()),
        });
    }

    let has_chrome = document.contains("behindDoc=\"1\"")
        || (document.contains("<w:pict>")
            && (document.contains("z-index:-") || document.contains("z-index: -")));
    if has_chrome {
        traits.push(DetectedTrait {
            id: "background:chrome".into(),
            kind: TraitKind::Background,
            label: "Background · behind-doc chrome".into(),
            value: "DrawingML / VML behind text".into(),
            style_id: None,
            swatch: background_fill.clone(),
        });
    }

    // Stable order: backgrounds, fonts, colours — then by id.
    traits.sort_by(|a, b| {
        let rank = |k: TraitKind| match k {
            TraitKind::Background => 0,
            TraitKind::Font => 1,
            TraitKind::Colour => 2,
        };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });
    traits.dedup_by(|a, b| a.id == b.id);

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("template.docx")
        .to_string();

    Ok(TemplatePreview {
        path: path.display().to_string(),
        file_name,
        format: "docx".into(),
        summary: TemplatePreviewSummary {
            fonts: fonts_seen.into_iter().collect(),
            colours: colours_seen.into_iter().collect(),
            background_fill,
            has_background_chrome: has_chrome,
            style_count,
        },
        traits,
        limits: Vec::new(),
    })
}

/// Convenience: inspect then map selection → options (re-detects traits).
pub fn options_from_template(
    template: &Path,
    selection: &TraitSelection,
) -> Result<(TemplatePreview, TransformOptions)> {
    let preview = inspect_template(template)?;
    let opts = TransformOptions::from_selection(&preview.traits, selection);
    Ok((preview, opts))
}

fn iter_style_blocks(styles_xml: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let mut rest = styles_xml;
    while let Some(rel) = rest.find("<w:style") {
        let from = &rest[rel..];
        let Some(end) = find_named_element_end(from, "w:style") else {
            break;
        };
        let block = &from[..end];
        if let Some(id) = attr_val(block, "w:styleId") {
            // Prefer paragraph / character styles with rPr; skip empty shells.
            out.push((id, block));
        }
        rest = &from[end..];
    }
    out
}

fn first_attr_in(block: &str, element: &str, attr: &str) -> Option<String> {
    let open = format!("<{element}");
    let at = block.find(&open)?;
    let from = &block[at..];
    let end = find_named_element_end(from, element)?;
    attr_val(&from[..end], attr)
}

fn attr_val(tag_or_block: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let at = tag_or_block.find(&needle)?;
    let v = &tag_or_block[at + needle.len()..];
    let end = v.find('"')?;
    Some(v[..end].to_string())
}

fn find_named_element_end(xml: &str, local: &str) -> Option<usize> {
    let first_gt = xml.find('>')?;
    if xml[..first_gt].ends_with('/')
        || xml.as_bytes().get(first_gt.saturating_sub(1)) == Some(&b'/')
    {
        return Some(first_gt + 1);
    }
    let close = format!("</{local}>");
    let close_at = xml.find(&close)?;
    Some(close_at + close.len())
}

fn normalize_hex(raw: &str) -> Option<String> {
    let s = raw.trim().trim_start_matches('#').to_ascii_uppercase();
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(s)
    } else if s.len() == 3 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        // Expand short hex
        let mut out = String::with_capacity(6);
        for c in s.chars() {
            out.push(c);
            out.push(c);
        }
        Some(out)
    } else {
        None
    }
}

fn document_background_hex(document: &str) -> Option<String> {
    let at = document.find("<w:background")?;
    let from = &document[at..];
    let gt = from.find('>')?;
    attr_val(&from[..=gt], "w:color").and_then(|v| normalize_hex(&v))
}

fn theme_latin_fonts(theme: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (slot, tag) in [("major", "a:majorFont"), ("minor", "a:minorFont")] {
        if let Some(at) = theme.find(&format!("<{tag}>")) {
            let from = &theme[at..];
            if let Some(end) = from.find(&format!("</{tag}>")) {
                let block = &from[..end];
                if let Some(tf) = attr_val(block, "typeface") {
                    if !tf.is_empty() {
                        out.push((slot.to_string(), tf));
                    }
                }
            }
        }
    }
    out
}

fn theme_scheme_colours(theme: &str) -> Vec<(String, String)> {
    let names = [
        "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5",
        "accent6", "hlink", "folHlink",
    ];
    let mut out = Vec::new();
    for name in names {
        let open = format!("<a:{name}>");
        let close = format!("</a:{name}>");
        let Some(at) = theme.find(&open) else {
            continue;
        };
        let from = &theme[at..];
        let Some(end) = from.find(&close) else {
            continue;
        };
        let block = &from[..end];
        // Prefer srgbClr; fall back to sysClr lastClr.
        let hex = attr_after(block, "<a:srgbClr", "val")
            .or_else(|| attr_after(block, "<a:sysClr", "lastClr"))
            .and_then(|v| normalize_hex(&v));
        if let Some(h) = hex {
            out.push((name.to_string(), h));
        }
    }
    out
}

fn attr_after(block: &str, needle: &str, attr: &str) -> Option<String> {
    let at = block.find(needle)?;
    let from = &block[at..];
    let gt = from.find('>')?;
    attr_val(&from[..=gt], attr)
}

/// Scrub fonts and/or colours out of a styles part when those traits are not selected.
pub(crate) fn scrub_styles_for_options(styles_xml: &str, opts: TransformOptions) -> String {
    if opts.apply_fonts && opts.apply_colours {
        return styles_xml.to_string();
    }
    let mut out = styles_xml.to_string();
    if !opts.apply_fonts {
        out = strip_elements(&out, &["w:rFonts", "w:sz", "w:szCs"]);
    }
    if !opts.apply_colours {
        out = strip_elements(&out, &["w:color"]);
        // themeColor on colour elements already removed with the element; also drop
        // leftover themeColor-only attributes on other nodes is rare — skip.
    }
    out
}

/// Remove document `w:background` when background traits are not applied.
pub(crate) fn strip_document_background(document_xml: &str) -> String {
    let Some(at) = document_xml.find("<w:background") else {
        return document_xml.to_string();
    };
    let from = &document_xml[at..];
    let end = if let Some(close) = from.find("</w:background>") {
        close + "</w:background>".len()
    } else if let Some(gt) = from.find('>') {
        gt + 1
    } else {
        return document_xml.to_string();
    };
    format!("{}{}", &document_xml[..at], &from[end..])
}

fn strip_elements(xml: &str, names: &[&str]) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
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
        if names.iter().any(|n| *n == name) {
            if let Some(end) = find_named_element_end(from, name) {
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

/// Selective presentation strip for source body content.
pub(crate) fn strip_presentation_for_options(xml: &str, opts: TransformOptions) -> String {
    if !opts.apply_fonts && !opts.apply_colours {
        // Still strip sizes only when fonts apply; here neither → leave source rPr alone
        // except we may still want to strip nothing.
        return xml.to_string();
    }
    let mut strip: Vec<&str> = Vec::new();
    if opts.apply_colours {
        strip.extend_from_slice(&["w:color", "w:highlight", "w:shd", "w:em"]);
    }
    if opts.apply_fonts {
        strip.extend_from_slice(&[
            "w:rFonts",
            "w:sz",
            "w:szCs",
            "w:spacing",
            "w:w",
            "w:kern",
            "w:position",
            "w:snapToGrid",
            "w:vanish",
            "w:bdr",
        ]);
    }
    strip_rpr_children(xml, &strip)
}

fn strip_rpr_children(xml: &str, strip: &[&str]) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<w:rPr") {
        out.push_str(&rest[..start]);
        let from = &rest[start..];
        let Some(end) = find_named_element_end(from, "w:rPr") else {
            out.push_str(from);
            return out;
        };
        let rpr = &from[..end];
        out.push_str(&clean_rpr_keeping(rpr, strip));
        rest = &from[end..];
    }
    out.push_str(rest);
    out
}

fn clean_rpr_keeping(rpr: &str, strip: &[&str]) -> String {
    let first_gt = match rpr.find('>') {
        Some(i) => i,
        None => return rpr.to_string(),
    };
    if rpr.as_bytes().get(first_gt.saturating_sub(1)) == Some(&b'/') {
        return String::new();
    }
    let inner = &rpr[first_gt + 1..rpr.len() - "</w:rPr>".len()];
    let cleaned = {
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
            if strip.iter().any(|s| *s == name) {
                if let Some(end) = find_named_element_end(from, name) {
                    rest = &from[end..];
                    continue;
                }
            }
            out.push('<');
            rest = &from[1..];
        }
        out.push_str(rest);
        out
    };
    if cleaned.chars().all(|c| c.is_whitespace()) {
        return String::new();
    }
    format!("<w:rPr>{cleaned}</w:rPr>")
}

/// Group trait ids by kind (used by CLI summaries / tests).
pub fn traits_by_kind(traits: &[DetectedTrait]) -> BTreeMap<TraitKind, Vec<&DetectedTrait>> {
    let mut map: BTreeMap<TraitKind, Vec<&DetectedTrait>> = BTreeMap::new();
    for t in traits {
        map.entry(t.kind).or_default().push(t);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{write_minimal_docx, SAMPLE_STYLES, TEMPLATE_STYLES};
    use std::path::PathBuf;

    #[test]
    fn inspect_minimal_template_detects_fonts_and_colours() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-test-traits");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("master.docx");
        write_minimal_docx(
            &path,
            "Heading1",
            "Normal",
            &[("title", "T"), ("body", "B")],
            TEMPLATE_STYLES,
            None,
        )
        .unwrap();

        let preview = inspect_template(&path).unwrap();
        assert!(preview.summary.fonts.iter().any(|f| f == "Calibri"));
        assert!(preview.summary.colours.iter().any(|c| c == "C45911"));
        assert!(preview.summary.colours.iter().any(|c| c == "1F4E79"));
        assert!(preview.traits.iter().any(|t| t.id == "font:style:Normal"));
        assert!(preview
            .traits
            .iter()
            .any(|t| t.id == "colour:style:Heading1"));
        assert!(preview.traits.iter().any(|t| t.kind == TraitKind::Background));
    }

    #[test]
    fn inspect_real_master_fixture_detects_chrome_and_fill() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("spike/fixtures/master-template-example.docx");
        assert!(path.exists());
        let preview = inspect_template(&path).unwrap();
        assert!(
            preview.summary.has_background_chrome,
            "expected behind-doc chrome on Beau master fixture"
        );
        assert_eq!(
            preview.summary.background_fill.as_deref(),
            Some("FDFBF2")
        );
        assert!(preview.traits.iter().any(|t| t.id == "background:fill"));
        assert!(preview.traits.iter().any(|t| t.id == "background:chrome"));
        assert!(preview.summary.fonts.iter().any(|f| f.contains("Gill") || f == "Aharoni"));
    }

    #[test]
    fn selection_maps_to_transform_options() {
        let traits = vec![
            DetectedTrait {
                id: "font:style:Normal".into(),
                kind: TraitKind::Font,
                label: "Font".into(),
                value: "Calibri".into(),
                style_id: Some("Normal".into()),
                swatch: None,
            },
            DetectedTrait {
                id: "colour:style:Heading1".into(),
                kind: TraitKind::Colour,
                label: "Colour".into(),
                value: "#C45911".into(),
                style_id: Some("Heading1".into()),
                swatch: Some("C45911".into()),
            },
            DetectedTrait {
                id: "background:fill".into(),
                kind: TraitKind::Background,
                label: "Background".into(),
                value: "#FDFBF2".into(),
                style_id: None,
                swatch: Some("FDFBF2".into()),
            },
        ];

        assert_eq!(
            TransformOptions::from_selection(&traits, &TraitSelection { selected_ids: None }),
            TransformOptions::all()
        );
        assert_eq!(
            TransformOptions::from_selection(
                &traits,
                &TraitSelection {
                    selected_ids: Some(vec![])
                }
            ),
            TransformOptions::none()
        );
        let only_colour = TransformOptions::from_selection(
            &traits,
            &TraitSelection {
                selected_ids: Some(vec!["colour:style:Heading1".into()]),
            },
        );
        assert!(!only_colour.apply_fonts);
        assert!(only_colour.apply_colours);
        assert!(!only_colour.apply_background);
    }

    #[test]
    fn options_csv_parses() {
        assert_eq!(TransformOptions::from_csv("all").unwrap(), TransformOptions::all());
        assert_eq!(TransformOptions::from_csv("none").unwrap(), TransformOptions::none());
        let o = TransformOptions::from_csv("fonts,background").unwrap();
        assert!(o.apply_fonts && o.apply_background && !o.apply_colours);
    }

    #[test]
    fn scrub_and_strip_helpers_respect_flags() {
        let styles = SAMPLE_STYLES;
        let no_fonts = scrub_styles_for_options(
            styles,
            TransformOptions {
                apply_fonts: false,
                apply_colours: true,
                apply_background: true,
            },
        );
        assert!(!no_fonts.contains("w:rFonts"));
        assert!(no_fonts.contains("w:color"));

        let body = r#"<w:r><w:rPr><w:b/><w:color w:val="000000"/><w:rFonts w:ascii="Arial"/><w:sz w:val="24"/></w:rPr><w:t>Hi</w:t></w:r>"#;
        let colours_only = strip_presentation_for_options(
            body,
            TransformOptions {
                apply_fonts: false,
                apply_colours: true,
                apply_background: false,
            },
        );
        assert!(!colours_only.contains("w:color"));
        assert!(colours_only.contains("w:rFonts"));
        assert!(colours_only.contains("<w:b/>"));
    }
}
