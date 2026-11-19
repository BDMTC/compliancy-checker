# Compliancy Checker

Windows-first **Tauri** desktop app that ingests `.docx` files, applies selectable rulesets, and exports restyled copies while **preserving factual content**.

Two modes, one codebase:

| Mode | Network / AI | Scope |
|---|---|---|
| **Standard** | None required | Template style merge + ISO section checklist export + (stub) structured context |
| **Assist** | Optional LLM API hook (stubbed) | Same core + future semantic ISO / freeform context |

## Status

- Tauri + Vite (TypeScript) app shell with ruleset UI, mode toggle, pickers, Run + log.
- Rust Open XML `.docx` transform with fixtures and CLI.
- **ISO checklist (Standard):** selectable 9001 / 14001 / 45001 section/clause presence against ingested `.docx` text; writes `iso-checklist.csv` + `iso-checklist-report.txt` next to processed docs.
- Assist provider config UI + Rust trait/stub — **no live API calls**.
- Structured context rules: noted in the run log only (not applied yet).

See [crates/docx-core/spike/README.md](crates/docx-core/spike/README.md) for transform spike details and ISO CLI usage.

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

# Unit tests (transform + checklist)
cargo test -p docx-core

# Full desktop app (needs platform WebView deps)
npm run tauri dev
```

### App UI flow

1. Choose **Standard** or **Assist**.
2. Toggle rulesets: ISO standards, master template path, context text.
3. Pick input `.docx` files or a folder; pick output directory.
4. **Run pipeline** — template-enabled runs use the Rust style merge; with ISO on, checklist CSV/report are written beside outputs. Assist semantic steps log a stub warning.

## Pipeline

```
ingest files/path → extract content → apply template styles → apply context (stub) → ISO checklist → export
```

ISO checklist is **presence-only** (clause numbers / section title keywords in document text). It assists review; it does **not** certify accreditation. Assist-mode LLM gap narrative remains stubbed.

## Project layout

```
├── src/                         # Vite + TypeScript UI
├── crates/docx-core/            # Open XML merge, text extract, ISO checklist
│   └── spike/                   # Fixtures, output, spike README
├── src-tauri/                   # Tauri shell, pipeline, Assist stub
└── README.md
```

## Licence

Proprietary — BDMTC. All rights reserved unless otherwise noted.
