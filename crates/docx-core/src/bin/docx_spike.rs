//! CLI for the `.docx` transform spike, ISO checklist, and context rules (no GUI).
//!
//! Usage:
//!   cargo run -p docx-core --bin docx-spike -- generate-fixtures
//!   cargo run -p docx-core --bin docx-spike -- run
//!   cargo run -p docx-core --bin docx-spike -- run --source PATH --template PATH --output PATH
//!   cargo run -p docx-core --bin docx-spike -- run --traits fonts,colours,background
//!   cargo run -p docx-core --bin docx-spike -- inspect-template --template PATH
//!   cargo run -p docx-core --bin docx-spike -- iso-checklist --source PATH --standards 45001,9001 --output-dir PATH
//!   cargo run -p docx-core --bin docx-spike -- context-rules --source PATH --rules PATH --output-dir PATH

use std::env;
use std::path::{Path, PathBuf};
use std::process;

use docx_core::{
    apply_context_to_docx, evaluate_docx, inspect_template, parse_context_rules, parse_standards,
    transform_docx_with_options, write_context_exports, write_exports, write_minimal_docx,
    TransformOptions, SAMPLE_STYLES, TEMPLATE_STYLES,
};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures")
}

fn generate_fixtures(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let sample = dir.join("sample-input.docx");
    let template = dir.join("master-template.docx");
    let iso_partial = dir.join("iso-45001-partial.docx");

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

    // Partial OH&S manual — enough headings for several 45001 PASSes and deliberate MISSes.
    write_minimal_docx(
        &iso_partial,
        "Heading1",
        "Normal",
        &[
            ("title", "OH&S Management System Manual (partial)"),
            ("title", "4.1 Context of the organization"),
            (
                "body",
                "External and internal issues that affect OH&S performance are reviewed annually.",
            ),
            ("title", "4.2 Needs and expectations of workers and other interested parties"),
            ("body", "Workers, contractors, and regulators are considered interested parties."),
            ("title", "5.1 Leadership and commitment"),
            ("body", "Top management demonstrates leadership and commitment to OH&S."),
            ("title", "5.2 OH&S policy"),
            ("body", "The health and safety policy is communicated and available."),
            ("title", "5.4 Consultation and participation of workers"),
            ("body", "Worker participation forums meet quarterly."),
            ("title", "6.1.2 Hazard identification and assessment of risks and opportunities"),
            ("body", "Hazard identification covers routine and non-routine activities."),
            ("title", "7.5 Documented information"),
            ("body", "Documented information is controlled per procedure DOC-01."),
            ("title", "8.2 Emergency preparedness and response"),
            ("body", "Emergency preparedness drills are scheduled annually."),
            ("title", "9.2 Internal audit"),
            ("body", "Internal audit programme covers all departments."),
            ("title", "10.2 Incident, nonconformity and corrective action"),
            ("body", "Incidents trigger nonconformity and corrective action records."),
        ],
        SAMPLE_STYLES,
        None,
    )
    .map_err(|e| e.to_string())?;

    println!("Wrote {}", sample.display());
    println!("Wrote {}", template.display());
    println!("Wrote {}", iso_partial.display());
    Ok(())
}

fn run_spike(
    source: &Path,
    template: &Path,
    output: &Path,
    opts: TransformOptions,
) -> Result<(), String> {
    if !source.exists() {
        return Err(format!("source not found: {}", source.display()));
    }
    if !template.exists() {
        return Err(format!("template not found: {}", template.display()));
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    transform_docx_with_options(source, template, output, opts).map_err(|e| e.to_string())?;
    println!("OK — wrote {}", output.display());
    println!(
        "Applied traits: fonts={}, colours={}, background={}",
        opts.apply_fonts, opts.apply_colours, opts.apply_background
    );
    println!(
        "Open in Word/LibreOffice: factual text from source; selected template traits applied."
    );
    Ok(())
}

fn run_inspect_template(template: &Path) -> Result<(), String> {
    if !template.exists() {
        return Err(format!("template not found: {}", template.display()));
    }
    let preview = inspect_template(template).map_err(|e| e.to_string())?;
    println!("Template: {}", preview.path);
    println!(
        "Summary: {} trait(s), {} style(s), fonts={:?}, colours={}, fill={:?}, chrome={}",
        preview.traits.len(),
        preview.summary.style_count,
        preview.summary.fonts,
        preview.summary.colours.len(),
        preview.summary.background_fill,
        preview.summary.has_background_chrome
    );
    for t in &preview.traits {
        println!(
            "  [{}] {} — {} ({})",
            t.kind.as_str(),
            t.id,
            t.label,
            t.value
        );
    }
    Ok(())
}

fn run_iso_checklist(
    source: &Path,
    standards_csv: &str,
    output_dir: &Path,
) -> Result<(), String> {
    if !source.exists() {
        return Err(format!("source not found: {}", source.display()));
    }
    let raw: Vec<String> = standards_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let standards = parse_standards(&raw).map_err(|e| e.to_string())?;
    if standards.is_empty() {
        return Err("provide at least one standard (9001, 14001, 45001)".into());
    }
    let report = evaluate_docx(source, &standards).map_err(|e| e.to_string())?;
    let (csv_path, txt_path) = write_exports(&report, output_dir).map_err(|e| e.to_string())?;
    println!(
        "ISO checklist: {} present, {} missing",
        report.present_count(),
        report.missing_count()
    );
    println!("Wrote {csv_path}");
    println!("Wrote {txt_path}");
    Ok(())
}

fn run_context_rules(
    source: &Path,
    rules_path: &Path,
    output_dir: &Path,
) -> Result<(), String> {
    if !source.exists() {
        return Err(format!("source not found: {}", source.display()));
    }
    if !rules_path.exists() {
        return Err(format!("rules file not found: {}", rules_path.display()));
    }
    std::fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;

    let dest = output_dir.join(
        source
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("context-output.docx"),
    );
    std::fs::copy(source, &dest).map_err(|e| e.to_string())?;

    let raw = std::fs::read_to_string(rules_path).map_err(|e| e.to_string())?;
    let rules = parse_context_rules(&raw).map_err(|e| e.to_string())?;
    if rules.is_empty() {
        return Err("rules file produced zero rules".into());
    }

    let report = apply_context_to_docx(&dest, &rules).map_err(|e| e.to_string())?;
    let (csv_path, txt_path) =
        write_context_exports(&report, output_dir).map_err(|e| e.to_string())?;
    println!(
        "Context rules: {} applied/replaced, {} required misses",
        report.applied_count(),
        report.missing_count()
    );
    println!("Wrote {}", dest.display());
    println!("Wrote {csv_path}");
    println!("Wrote {txt_path}");
    Ok(())
}

fn print_usage() {
    eprintln!(
        "docx-spike — Compliancy Checker Open XML helpers

Commands:
  generate-fixtures          Write sample, template, and iso-45001-partial.docx under spike/fixtures
  run [options]              Transform fixtures (or custom paths) to an output .docx
  inspect-template [options] Detect fonts / colours / backgrounds for the trait picker
  iso-checklist [options]    Section/clause presence checklist → CSV + text report
  context-rules [options]    Apply structured context rules → mutated .docx + report

Options for run:
  --source PATH              Default: crates/docx-core/spike/fixtures/sample-input.docx
  --template PATH            Default: crates/docx-core/spike/fixtures/master-template.docx
  --output PATH              Default: crates/docx-core/spike/out/styled-output.docx
  --traits LIST              fonts,colours,background (default: all) — or none

Options for inspect-template:
  --template PATH            Default: crates/docx-core/spike/fixtures/master-template-example.docx

Options for iso-checklist:
  --source PATH              Default: crates/docx-core/spike/fixtures/iso-45001-partial.docx
  --standards LIST           Comma-separated: 9001,14001,45001 (default: 45001)
  --output-dir PATH          Default: crates/docx-core/spike/out

Options for context-rules:
  --source PATH              Default: crates/docx-core/spike/fixtures/sample-input.docx
  --rules PATH               Default: crates/docx-core/spike/fixtures/context-rules.example.txt
  --output-dir PATH          Default: crates/docx-core/spike/out
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
            let mut traits_csv = "all".to_string();

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
                    "--traits" => {
                        i += 1;
                        traits_csv = args
                            .get(i)
                            .expect("--traits needs a value")
                            .clone();
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

            let opts = match TransformOptions::from_csv(&traits_csv) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(2);
                }
            };
            run_spike(&source, &template, &output, opts)
        }
        "inspect-template" => {
            let mut template = fixtures.join("master-template-example.docx");
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--template" => {
                        i += 1;
                        template = PathBuf::from(args.get(i).expect("--template needs a path"));
                    }
                    other => {
                        eprintln!("Unknown argument: {other}");
                        print_usage();
                        process::exit(2);
                    }
                }
                i += 1;
            }
            if !template.exists() {
                template = fixtures.join("master-template.docx");
            }
            run_inspect_template(&template)
        }
        "iso-checklist" => {
            let mut source = fixtures.join("iso-45001-partial.docx");
            let mut standards = "45001".to_string();
            let mut output_dir =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/out");

            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--source" => {
                        i += 1;
                        source = PathBuf::from(args.get(i).expect("--source needs a path"));
                    }
                    "--standards" => {
                        i += 1;
                        standards = args
                            .get(i)
                            .expect("--standards needs a value")
                            .clone();
                    }
                    "--output-dir" => {
                        i += 1;
                        output_dir =
                            PathBuf::from(args.get(i).expect("--output-dir needs a path"));
                    }
                    other => {
                        eprintln!("Unknown argument: {other}");
                        print_usage();
                        process::exit(2);
                    }
                }
                i += 1;
            }

            if !source.exists() {
                if let Err(e) = generate_fixtures(&fixtures) {
                    eprintln!("Failed to generate fixtures: {e}");
                    process::exit(1);
                }
                source = fixtures.join("iso-45001-partial.docx");
            }

            run_iso_checklist(&source, &standards, &output_dir)
        }
        "context-rules" => {
            let mut source = fixtures.join("sample-input.docx");
            let mut rules = fixtures.join("context-rules.example.txt");
            let mut output_dir =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/out");

            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--source" => {
                        i += 1;
                        source = PathBuf::from(args.get(i).expect("--source needs a path"));
                    }
                    "--rules" => {
                        i += 1;
                        rules = PathBuf::from(args.get(i).expect("--rules needs a path"));
                    }
                    "--output-dir" => {
                        i += 1;
                        output_dir =
                            PathBuf::from(args.get(i).expect("--output-dir needs a path"));
                    }
                    other => {
                        eprintln!("Unknown argument: {other}");
                        print_usage();
                        process::exit(2);
                    }
                }
                i += 1;
            }

            if !source.exists() {
                if let Err(e) = generate_fixtures(&fixtures) {
                    eprintln!("Failed to generate fixtures: {e}");
                    process::exit(1);
                }
                source = fixtures.join("sample-input.docx");
            }

            run_context_rules(&source, &rules, &output_dir)
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
