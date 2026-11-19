# Compliancy Checker

Windows-first **Tauri** desktop app that ingests `.docx` files, applies selectable rulesets, and exports restyled copies while **preserving factual content**.

Two modes, one codebase:

| Mode | Network / AI | Scope |
|---|---|---|
| **Standard** | None required | Template style merge + structured context rules + ISO section checklist |
| **Assist** | Optional LLM API hook (stubbed) | Same core + future semantic ISO / freeform context |

## Status

- Tauri + Vite (TypeScript) app shell with ruleset UI, mode toggle, pickers, Run + log.
- Rust Open XML `.docx` transform with fixtures and CLI.
- **ISO checklist (Standard):** selectable 9001 / 14001 / 45001 section/clause presence against ingested `.docx` text; writes `iso-checklist.csv` + `iso-checklist-report.txt` next to processed docs.
- **Structured context rules (Standard):** deterministic line/JSON rule language (force heading, find/replace, require section/heading); applied after template merge; writes `context-rules.csv` + `context-rules-report.txt`.
- Assist provider config UI + Rust trait/stub — **no live API calls**.

See [crates/docx-core/spike/README.md](crates/docx-core/spike/README.md) for transform spike details, ISO CLI, and context-rules CLI usage.

## Requirements

### Windows (primary target)

- [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalled on recent Windows 10/11)
- [Rust](https://rustup.rs/) stable (`rustc`, `cargo`)
- [Node.js](https://nodejs.org/) 20+ (npm)
- Visual Studio Build Tools with the C++ workload (to compile Tauri on Windows)

### Linux (dev / CI)

- Rust + Node as above
- Tauri system packages (`webkit2gtk`, `librsvg2`, …) — see [Tauri prerequisites](https://tauri.app/start/prerequisites/)

### macOS

- Rust + Node; Xcode CLT. Officially Windows-first; macOS is best-effort only.

## Quick start

```bash
npm install

# Docx spike only (no GUI / WebView required)
npm run spike:fixtures
npm run spike:run
# → crates/docx-core/spike/out/styled-output.docx

# ISO checklist against the partial OH&S fixture (no GUI)
npm run spike:iso
# → crates/docx-core/spike/out/iso-checklist.csv
# → crates/docx-core/spike/out/iso-checklist-report.txt

# Structured context rules against sample-input.docx (no GUI)
npm run spike:context
# → crates/docx-core/spike/out/sample-input.docx (mutated copy)
# → crates/docx-core/spike/out/context-rules.csv
# → crates/docx-core/spike/out/context-rules-report.txt

# Unit tests (transform + checklist + context rules)
cargo test -p docx-core

# Full desktop app (needs platform WebView deps)
npm run tauri dev
```

### App UI flow

1. Choose **Standard** or **Assist**.
2. Toggle rulesets: ISO standards, master template path, context rules text.
3. Pick input `.docx` files or a folder; pick output directory.
4. **Run pipeline** — template style merge → structured context apply → ISO checklist exports. Assist semantic steps log a stub warning.

## Pipeline

```
ingest files/path → extract content → apply template styles → apply context rules → ISO checklist → export
```

ISO checklist is **presence-only** (clause numbers / section title keywords). Context rules are a **fixed rule language** (not freeform LLM). Neither certifies accreditation. Assist-mode LLM gap narrative remains stubbed.

## Context rule syntax

Enable **Context rules** and paste either **line-based** rules or a **JSON** document. The Context panel is **batch-global** by default; use `document:` (or JSON `"document"`) to scope rules to a basename.

### Line-based

```text
# Comments start with #
force_heading: Revision History
require_section: Document Control
require_heading: Scope
replace: Acme Works Ltd => ACME Works Limited

# Optional per-document scope (basename match).
# `document: *` or bare `document:` returns to batch-global.
document: site-report.docx
replace: DRAFT => FINAL
```

| Op | Effect |
|---|---|
| `force_heading` / `force_section` | Ensure a Heading1 with this text exists; append if missing |
| `require_section` | Presence check in body/headings (beyond ISO checklist) |
| `require_heading` | Presence check (prefer headings) |
| `replace` | Literal find → replace inside Word run text (`w:t`; not cross-run) |
| `document:` | Scope following rules to that file basename |

### JSON

If the field (or rules file) starts with `{`, it is parsed as:

```json
{
  "rules": [
    { "op": "force_heading", "text": "Revision History" },
    { "op": "require_section", "text": "Document Control" },
    { "op": "replace", "find": "Acme", "with": "ACME Ltd" },
    { "op": "replace", "find": "DRAFT", "with": "FINAL", "document": "site.docx" }
  ]
}
```

Example file: [crates/docx-core/spike/fixtures/context-rules.example.txt](crates/docx-core/spike/fixtures/context-rules.example.txt).

Missing `require_*` checks fail the run (`ok: false`) and are listed in the context report exports.

## Project layout

```
├── src/                         # Vite + TypeScript UI
├── crates/docx-core/            # Open XML merge, extract, ISO, context rules
│   └── spike/                   # Fixtures, output, spike README
├── src-tauri/                   # Tauri shell, pipeline, Assist stub
└── README.md
```

## Licence

Proprietary — BDMTC. All rights reserved unless otherwise noted.
