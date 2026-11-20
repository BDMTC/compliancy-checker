//! CLI for PDF master inspect + PDF export fixtures (no GUI).
//!
//! Usage:
//!   cargo run -p pdf-core --bin pdf-spike -- generate-fixtures
//!   cargo run -p pdf-core --bin pdf-spike -- inspect-template --template PATH
//!   cargo run -p pdf-core --bin pdf-spike -- export --source PATH.docx --output PATH.pdf

use std::env;
use std::path::{Path, PathBuf};
use std::process;

use pdf_core::{
    cues_from_preview, inspect_any_template, write_pdf_from_blocks, write_pdf_from_docx,
    PdfExportOptions, PdfStyleCues, PDF_LIMITATIONS,
};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures")
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/out")
}

fn generate_fixtures() -> Result<(), String> {
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let master = dir.join("master-template.pdf");
    write_pdf_from_blocks(
        &master,
        &[
            ("heading", "Master PDF Template"),
            (
                "body",
                "Brand body sample. Fonts Helvetica; heading #C45911; body #1F4E79; fill #F5F7FA.",
            ),
        ],
        &PdfExportOptions {
            title: "Master PDF Template".into(),
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
    .map_err(|e| e.to_string())?;
    println!("Wrote {}", master.display());
    println!("PDF master limits:");
    for line in PDF_LIMITATIONS {
        println!("  - {line}");
    }
    Ok(())
}

fn inspect(path: &Path) -> Result<(), String> {
    let preview = inspect_any_template(path).map_err(|e| e.to_string())?;
    println!("format: {}", preview.format);
    println!("file: {}", preview.file_name);
    println!("traits: {}", preview.traits.len());
    for t in &preview.traits {
        println!("  [{}] {} = {}", t.kind.as_str(), t.label, t.value);
    }
    if !preview.limits.is_empty() {
        println!("limits:");
        for l in &preview.limits {
            println!("  - {l}");
        }
    }
    Ok(())
}

fn export(source: &Path, output: &Path, template: Option<&Path>) -> Result<(), String> {
    let opts = if let Some(tpl) = template {
        let preview = inspect_any_template(tpl).map_err(|e| e.to_string())?;
        let transform = docx_core::TransformOptions::all();
        PdfExportOptions {
            title: source
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("export")
                .into(),
            cues: cues_from_preview(&preview, &transform),
            apply_fonts: true,
            apply_colours: true,
            apply_background: true,
        }
    } else {
        PdfExportOptions::default()
    };
    write_pdf_from_docx(source, output, &opts).map_err(|e| e.to_string())?;
    println!("Wrote {}", output.display());
    Ok(())
}

fn print_help() {
    eprintln!(
        "pdf-spike commands:
  generate-fixtures
  inspect-template --template PATH
  export --source PATH.docx --output PATH.pdf [--template PATH]"
    );
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_help();
        process::exit(2);
    }
    let cmd = args.remove(0);
    let result = match cmd.as_str() {
        "generate-fixtures" => generate_fixtures(),
        "inspect-template" => {
            let template = take_flag(&mut args, "--template")
                .map(PathBuf::from)
                .unwrap_or_else(|| fixtures_dir().join("master-template.pdf"));
            inspect(&template)
        }
        "export" => {
            let source = take_flag(&mut args, "--source")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../docx-core/spike/fixtures/sample-input.docx")
                });
            let output = take_flag(&mut args, "--output")
                .map(PathBuf::from)
                .unwrap_or_else(|| out_dir().join("styled-output.pdf"));
            let template = take_flag(&mut args, "--template").map(PathBuf::from);
            let _ = std::fs::create_dir_all(out_dir());
            export(&source, &output, template.as_deref())
        }
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        other => Err(format!("unknown command '{other}'")),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.remove(pos);
    if pos < args.len() {
        Some(args.remove(pos))
    } else {
        None
    }
}
