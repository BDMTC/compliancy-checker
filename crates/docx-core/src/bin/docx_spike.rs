//! CLI for the `.docx` transform spike (no GUI required).
//!
//! Usage:
//!   cargo run -p docx-core --bin docx-spike -- generate-fixtures
//!   cargo run -p docx-core --bin docx-spike -- run
//!   cargo run -p docx-core --bin docx-spike -- run --source PATH --template PATH --output PATH

use std::env;
use std::path::{Path, PathBuf};
use std::process;

use docx_core::{transform_docx, write_minimal_docx, SAMPLE_STYLES, TEMPLATE_STYLES};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures")
}

fn generate_fixtures(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let sample = dir.join("sample-input.docx");
    let template = dir.join("master-template.docx");

    write_minimal_docx(
        &sample,
        "Heading1",
        "Normal",
        &[
            ("title", "Site Safety Report — Q3 Review"),
            (
                "body",
                "Customer: Acme Works Ltd. Address: 12 Harbour Road, Bristol BS1 1AA.",
            ),
            (
                "body",
                "Inspection date: 2026-03-14. Inspector: J. Patel. Finding: guardrail gap at bay 3; corrective action logged under CAR-441.",
            ),
            (
                "body",
                "This factual content must survive restyling unchanged.",
            ),
        ],
        SAMPLE_STYLES,
        None,
    )
    .map_err(|e| e.to_string())?;

    write_minimal_docx(
        &template,
        "Heading1",
        "Normal",
        &[
            ("title", "MASTER TEMPLATE — replace body"),
            (
                "body",
                "Template body placeholder. Styles: Calibri, heading accent C45911, body 1F4E79.",
            ),
        ],
        TEMPLATE_STYLES,
        None,
    )
    .map_err(|e| e.to_string())?;

    println!("Wrote {}", sample.display());
    println!("Wrote {}", template.display());
    Ok(())
}

fn run_spike(source: &Path, template: &Path, output: &Path) -> Result<(), String> {
    if !source.exists() {
        return Err(format!("source not found: {}", source.display()));
    }
    if !template.exists() {
        return Err(format!("template not found: {}", template.display()));
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    transform_docx(source, template, output).map_err(|e| e.to_string())?;
    println!("OK — wrote {}", output.display());
    println!(
        "Open in Word/LibreOffice: factual text from source, fonts/colours from template styles.xml."
    );
    Ok(())
}

fn print_usage() {
    eprintln!(
        "docx-spike — Compliancy Checker Open XML transform spike

Commands:
  generate-fixtures          Write sample-input.docx + master-template.docx under crates/docx-core/spike/fixtures
  run [options]              Transform fixtures (or custom paths) to an output .docx

Options for run:
  --source PATH              Default: crates/docx-core/spike/fixtures/sample-input.docx
  --template PATH            Default: crates/docx-core/spike/fixtures/master-template.docx
  --output PATH              Default: crates/docx-core/spike/out/styled-output.docx
"
    );
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        process::exit(2);
    }

    let cmd = args.remove(0);
    let fixtures = fixtures_dir();

    let result = match cmd.as_str() {
        "generate-fixtures" => generate_fixtures(&fixtures),
        "run" => {
            let mut source = fixtures.join("sample-input.docx");
            let mut template = fixtures.join("master-template.docx");
            let mut output =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/out/styled-output.docx");

            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--source" => {
                        i += 1;
                        source = PathBuf::from(args.get(i).expect("--source needs a path"));
                    }
                    "--template" => {
                        i += 1;
                        template = PathBuf::from(args.get(i).expect("--template needs a path"));
                    }
                    "--output" => {
                        i += 1;
                        output = PathBuf::from(args.get(i).expect("--output needs a path"));
                    }
                    other => {
                        eprintln!("Unknown argument: {other}");
                        print_usage();
                        process::exit(2);
                    }
                }
                i += 1;
            }

            if !source.exists() || !template.exists() {
                if let Err(e) = generate_fixtures(&fixtures) {
                    eprintln!("Failed to generate fixtures: {e}");
                    process::exit(1);
                }
                source = fixtures.join("sample-input.docx");
                template = fixtures.join("master-template.docx");
            }

            run_spike(&source, &template, &output)
        }
        "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
