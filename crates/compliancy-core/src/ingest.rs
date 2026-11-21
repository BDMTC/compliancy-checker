//! Mixed-document ingest: `.docx`, `.pdf`, legacy `.doc` (+ LibreOffice conversion).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Supported ingest kinds (extension-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Docx,
    Pdf,
    Doc,
}

impl InputKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Pdf => "pdf",
            Self::Doc => "doc",
        }
    }

    /// Default per-file output extension when Save as is "Match input".
    pub fn match_output_ext(self) -> &'static str {
        match self {
            Self::Docx | Self::Doc => "docx",
            Self::Pdf => "pdf",
        }
    }
}

/// One ingested document, tied to the input root it came from.
#[derive(Debug, Clone)]
pub struct CollectedInput {
    pub path: PathBuf,
    pub kind: InputKind,
    /// Folder path when walked from a directory; parent dir when a direct file pick.
    pub source_root: PathBuf,
    pub from_directory: bool,
}

/// Per input-root scan tallies (logged; supported types are never silently skipped).
#[derive(Debug, Clone, Default)]
pub struct RootScan {
    pub path: PathBuf,
    pub is_directory: bool,
    /// All non-hidden files seen in a directory listing (0 for direct file picks).
    pub total_files_in_dir: usize,
    pub docx: usize,
    pub pdf: usize,
    pub doc: usize,
    /// Unsupported extensions encountered while walking (logged as skipped).
    pub skipped_names: Vec<String>,
}

impl RootScan {
    pub fn supported_count(&self) -> usize {
        self.docx + self.pdf + self.doc
    }
}

#[derive(Debug, Clone)]
pub struct CollectResult {
    pub inputs: Vec<CollectedInput>,
    pub roots: Vec<RootScan>,
}

/// Classify by extension: docx / pdf / legacy doc. Returns `None` for unsupported.
pub fn classify_input(path: &Path) -> Option<InputKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "docx" => Some(InputKind::Docx),
        "pdf" => Some(InputKind::Pdf),
        "doc" => Some(InputKind::Doc),
        _ => None,
    }
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Collect `.docx` / `.pdf` / `.doc` from file picks and top-level folder walks.
pub fn collect_document_inputs(paths: &[String]) -> Result<CollectResult, String> {
    let mut inputs = Vec::new();
    let mut roots: Vec<RootScan> = Vec::new();

    for raw in paths {
        let p = PathBuf::from(raw);
        if !p.exists() {
            return Err(format!("input path not found: {}", p.display()));
        }

        if p.is_file() {
            let kind = classify_input(&p).ok_or_else(|| {
                format!(
                    "unsupported input type (need .docx, .pdf, or .doc): {}",
                    p.display()
                )
            })?;
            let parent = p
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let root_idx = ensure_root(&mut roots, &parent, false);
            match kind {
                InputKind::Docx => roots[root_idx].docx += 1,
                InputKind::Pdf => roots[root_idx].pdf += 1,
                InputKind::Doc => roots[root_idx].doc += 1,
            }
            inputs.push(CollectedInput {
                path: p,
                kind,
                source_root: roots[root_idx].path.clone(),
                from_directory: false,
            });
        } else if p.is_dir() {
            let root_idx = ensure_root(&mut roots, &p, true);
            let mut total = 0usize;
            for entry in fs::read_dir(&p).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if is_hidden(&name) {
                    continue;
                }
                total += 1;
                match classify_input(&path) {
                    Some(InputKind::Docx) => {
                        roots[root_idx].docx += 1;
                        inputs.push(CollectedInput {
                            path,
                            kind: InputKind::Docx,
                            source_root: p.clone(),
                            from_directory: true,
                        });
                    }
                    Some(InputKind::Pdf) => {
                        roots[root_idx].pdf += 1;
                        inputs.push(CollectedInput {
                            path,
                            kind: InputKind::Pdf,
                            source_root: p.clone(),
                            from_directory: true,
                        });
                    }
                    Some(InputKind::Doc) => {
                        roots[root_idx].doc += 1;
                        inputs.push(CollectedInput {
                            path,
                            kind: InputKind::Doc,
                            source_root: p.clone(),
                            from_directory: true,
                        });
                    }
                    None => {
                        roots[root_idx].skipped_names.push(name);
                    }
                }
            }
            roots[root_idx].total_files_in_dir = total;
        } else {
            return Err(format!("input path is neither file nor directory: {}", p.display()));
        }
    }

    inputs.sort_by(|a, b| a.path.cmp(&b.path));
    inputs.dedup_by(|a, b| a.path == b.path);
    Ok(CollectResult { inputs, roots })
}

fn ensure_root(roots: &mut Vec<RootScan>, path: &Path, is_directory: bool) -> usize {
    if let Some(i) = roots.iter().position(|r| r.path == path) {
        if is_directory {
            roots[i].is_directory = true;
        }
        return i;
    }
    roots.push(RootScan {
        path: path.to_path_buf(),
        is_directory,
        ..RootScan::default()
    });
    roots.len() - 1
}

/// Resolve Save-as preference to a concrete output extension for one input.
pub fn resolve_output_ext(output_format: &str, kind: InputKind) -> &'static str {
    let fmt = output_format.trim().to_ascii_lowercase();
    match fmt.as_str() {
        "pdf" => "pdf",
        "docx" | "word" => "docx",
        // "match" / "match-input" / empty → per-file match
        _ => kind.match_output_ext(),
    }
}

/// Locate LibreOffice / soffice for legacy `.doc` → `.docx` conversion.
pub fn find_soffice() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "soffice",
        "soffice.exe",
        "soffice.com",
        "libreoffice",
        "libreoffice.exe",
    ];
    for name in CANDIDATES {
        if let Ok(path) = which_cmd(name) {
            return Some(path);
        }
    }
    // Common Windows install locations (documented for operators without PATH).
    const WIN_PATHS: &[&str] = &[
        r"C:\Program Files\LibreOffice\program\soffice.exe",
        r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
        r"C:\Program Files\LibreOffice\program\soffice.com",
    ];
    for p in WIN_PATHS {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn which_cmd(name: &str) -> Result<PathBuf, ()> {
    let output = if cfg!(windows) {
        Command::new("where").arg(name).output()
    } else {
        Command::new("which").arg(name).output()
    };
    let output = output.map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return Err(());
    }
    let path = PathBuf::from(first);
    if path.is_file() {
        Ok(path)
    } else {
        Err(())
    }
}

/// Convert legacy `.doc` → `.docx` via LibreOffice headless.
///
/// Fails with a clear message when soffice is unavailable — callers must treat
/// that as **unsuccessful**, never a silent skip.
pub fn convert_doc_to_docx(doc_path: &Path, work_dir: &Path) -> Result<PathBuf, String> {
    let soffice = find_soffice().ok_or_else(|| {
        "Legacy .doc conversion unavailable: LibreOffice/soffice not found. \
         Install LibreOffice and ensure `soffice` is on PATH \
         (Windows: typically `C:\\Program Files\\LibreOffice\\program\\soffice.exe`), \
         then re-run. This file was not skipped silently."
            .to_string()
    })?;

    fs::create_dir_all(work_dir).map_err(|e| e.to_string())?;
    let stem = doc_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted");
    // Unique outdir so parallel/repeated runs do not clobber.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let out_subdir = work_dir.join(format!("doc-convert-{stem}-{nanos}"));
    fs::create_dir_all(&out_subdir).map_err(|e| e.to_string())?;

    let status = Command::new(&soffice)
        .args([
            "--headless",
            "--nologo",
            "--nolockcheck",
            "--nodefault",
            "--nofirststartwizard",
            "--convert-to",
            "docx",
            "--outdir",
        ])
        .arg(&out_subdir)
        .arg(doc_path)
        .status()
        .map_err(|e| format!("Failed to launch LibreOffice/soffice: {e}"))?;

    if !status.success() {
        return Err(format!(
            "LibreOffice conversion failed for {} (exit {:?})",
            doc_path.display(),
            status.code()
        ));
    }

    let expected = out_subdir.join(format!("{stem}.docx"));
    if expected.is_file() {
        return Ok(expected);
    }
    // LibreOffice may alter the stem; pick the first .docx written.
    for entry in fs::read_dir(&out_subdir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("docx") {
            return Ok(path);
        }
    }
    Err(format!(
        "LibreOffice reported success but no .docx appeared for {}",
        doc_path.display()
    ))
}

/// Count non-hidden files currently in an output directory.
pub fn count_files_in_dir(dir: &Path) -> usize {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            let Ok(ft) = e.file_type() else {
                return false;
            };
            if !ft.is_file() {
                return false;
            }
            let name = e.file_name();
            let name = name.to_string_lossy();
            !is_hidden(&name)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_core::{write_minimal_docx, SAMPLE_STYLES};
    use pdf_core::{write_pdf_from_blocks, PdfExportOptions, PdfStyleCues};

    #[test]
    fn collect_mixed_folder_counts_all_supported() {
        let dir = std::env::temp_dir().join("compliancy-ingest-mixed");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        write_minimal_docx(
            &dir.join("a.docx"),
            "Heading1",
            "Normal",
            &[("body", "hello")],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();
        write_pdf_from_blocks(
            &dir.join("b.pdf"),
            &[("body", "pdf body")],
            &PdfExportOptions {
                title: "t".into(),
                cues: PdfStyleCues::default(),
                apply_fonts: true,
                apply_colours: true,
                apply_background: false,
            },
        )
        .unwrap();
        // Legacy .doc fixture: opaque bytes (conversion tested separately).
        fs::write(dir.join("c.doc"), b"\xd0\xcf\x11\xe0 fake-ole-doc").unwrap();
        fs::write(dir.join("notes.txt"), b"skip me").unwrap();

        let result = collect_document_inputs(&[dir.display().to_string()]).unwrap();
        assert_eq!(result.inputs.len(), 3);
        assert_eq!(result.roots.len(), 1);
        let root = &result.roots[0];
        assert_eq!(root.total_files_in_dir, 4);
        assert_eq!(root.docx, 1);
        assert_eq!(root.pdf, 1);
        assert_eq!(root.doc, 1);
        assert_eq!(root.skipped_names, vec!["notes.txt".to_string()]);
        assert!(result.inputs.iter().any(|i| i.kind == InputKind::Doc));
    }

    #[test]
    fn match_output_ext_defaults() {
        assert_eq!(resolve_output_ext("match", InputKind::Pdf), "pdf");
        assert_eq!(resolve_output_ext("match", InputKind::Doc), "docx");
        assert_eq!(resolve_output_ext("match", InputKind::Docx), "docx");
        assert_eq!(resolve_output_ext("pdf", InputKind::Docx), "pdf");
        assert_eq!(resolve_output_ext("docx", InputKind::Pdf), "docx");
    }

    #[test]
    fn missing_soffice_fails_clearly() {
        // If soffice IS installed in CI, conversion may succeed on a fake file or fail
        // later — either way we only assert the unavailable path when find returns None.
        if find_soffice().is_some() {
            return;
        }
        let dir = std::env::temp_dir().join("compliancy-doc-noconvert");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("x.doc");
        fs::write(&doc, b"not-a-real-doc").unwrap();
        let err = convert_doc_to_docx(&doc, &dir.join("work")).unwrap_err();
        assert!(err.contains("not found") || err.contains("unavailable"), "{err}");
        assert!(err.contains("not skipped silently"), "{err}");
    }
}
