# Compliancy Checker

Windows-first **Tauri** desktop app that ingests `.docx` files, applies selectable rulesets, and exports restyled copies while **preserving factual content**.

Two modes, one codebase:

| Mode | Network / AI | Scope |
|---|---|---|
| **Standard** | None required — fully offline | Template style merge + structured context rules + ISO section checklist |
| **Assist** | Optional OpenAI-compatible LLM | Same core + ISO gap narrative + freeform context interpretation |

## Status

- Tauri + Vite (TypeScript) app shell with ruleset UI, mode toggle, pickers, Run + log.
- Rust Open XML `.docx` transform with fixtures and CLI.
- **Master template trait preview + picker:** on template load, detect fonts / colours / backgrounds; select all, one, or a subset before merge apply. Masters may be **`.docx` or `.pdf`**.
- **PDF export:** selectable **Save as** Word (`.docx`) or PDF (`.pdf`) beside the ingest controls.
- **ISO checklist (Standard):** selectable 9001 / 14001 / 45001 section/clause presence against ingested `.docx` text; writes `iso-checklist.csv` + `iso-checklist-report.txt` next to processed docs.
- **Structured context rules (Standard):** deterministic line/JSON rule language (force heading, find/replace, require section/heading); applied after template merge; writes `context-rules.csv` + `context-rules-report.txt`.
- **Assist live API hook:** OpenAI-compatible HTTP (OpenAI / Azure / local). Mock fixtures for CI. Standard mode never constructs a live client.

Follow-up (not this PR): **CC-12** ISO card **RUN FIRST** (checklist before pipeline; pass toast / fail modal + Print).

See [crates/docx-core/spike/README.md](crates/docx-core/spike/README.md) for transform spike details, and [crates/pdf-core/spike/README.md](crates/pdf-core/spike/README.md) for PDF master/export limits and CLI.

## Requirements

### Windows (primary target)

- [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalled on recent Windows 10/11)
- [Rust](https://rustup.rs/) stable (`rustc`, `cargo`)
- [Node.js](https://nodejs.org/) 20+ (npm)
- Visual Studio Build Tools with the C++ workload (to compile Tauri on Windows), **or** the no-admin path below

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

# PDF master fixture + inspect + export (no GUI)
npm run spike:pdf-fixtures
npm run spike:pdf-inspect
npm run spike:pdf-export
# → crates/pdf-core/spike/fixtures/master-template.pdf
# → crates/pdf-core/spike/out/styled-output.pdf

# Unit tests (transform + PDF + checklist + context rules + Assist mock; no WebView)
cargo test -p docx-core
cargo test -p pdf-core
cargo test -p compliancy-core

# Full desktop app (needs platform WebView deps)
npm run tauri dev
```

### Windows without VS Build Tools (xwin + portable LLVM)

If UAC / VS Build Tools is blocked, this machine can still build MSVC Tauri using:

- `C:\Temp\toolchains\xwin` — CRT/SDK splat from [xwin](https://github.com/Jake-Shadle/xwin)
- `C:\Temp\toolchains\llvm\clang+llvm-23.1.5-x86_64-pc-windows-msvc` — portable `clang-cl` / `lld-link` / `llvm-rc`
- Rust target `stable-x86_64-pc-windows-msvc`

Relaunch the GUI:

```powershell
powershell -ExecutionPolicy Bypass -File C:\Temp\compliancy-checker\scripts\tauri-dev-xwin.ps1
```

GNU/`x86_64-pc-windows-gnu` is not supported for this Tauri shell (export-ordinal link failure). Prefer real VS Build Tools C++ when UAC is available.

### App UI flow

1. Choose **Standard** or **Assist**.
2. Toggle rulesets: ISO standards, master template path, context rules text.
3. Browse a master **`.docx` or `.pdf`** — preview detected traits (fonts / colours / backgrounds / page size) and select all, one, or a subset before apply. PDF masters show documented cue limits in the panel.
4. In Assist: pick provider / model / endpoint; set API key (UI, env, or local config).
5. Pick input `.docx` files or a folder; pick output directory; choose **Save as** Word or PDF.
6. **Run pipeline** — template style merge (selected traits) → structured context apply → ISO checklist → (Assist only) gap narrative / freeform interpretation exports → write `.docx` or regenerated `.pdf`.

## Pipeline

```
ingest → extract → template styles → context rules → ISO checklist → [Assist: ISO gap + freeform] → export (.docx | .pdf)
```

ISO checklist is **presence-only** (clause numbers / section title keywords). Context rules are a **fixed rule language**. Assist adds optional LLM narrative only; it does not certify accreditation.

## PDF master + PDF export (CC-11)

| Capability | Behaviour |
|---|---|
| PDF as master template | Inspect BaseFont names, MediaBox page size, sampled content colours; trait picker same as Word. For Word output, cues become a **synthetic styles** `.docx` then Open XML merge. |
| PDF as save/output | After merge/context, regenerate a PDF from extracted text + selected cues (Type1 font mapping). Intermediate `.docx` is not kept when PDF is selected. |
| Inputs | Still `.docx` only (scanned PDF ingest / OCR is out of scope). |

**Limits:** not a binary restyle; no embedded-font subsetting; no headers/footers/images/multi-column from the PDF master; colour sampling is best-effort; scanned PDFs yield little; PDF export flattens tables/images to text. Details: [crates/pdf-core/spike/README.md](crates/pdf-core/spike/README.md).

## Assist mode (optional LLM)

Assist stays behind the mode toggle. **Standard never makes network calls**, even if an API key is present in the UI or environment.

### What Assist uses the LLM for

| Step | Output file | When |
|---|---|---|
| ISO gap narrative | `iso-gap-narrative.txt` | Assist + ISO checklist ran + provider configured |
| Freeform context interpretation | `assist-context-interpretation.txt` | Assist + context text has prose outside the structured rule language (or structured parse failed) |

Structured context ops still apply locally in both modes. Assist only interprets leftover freeform intent.

### Configuration (never commit secrets)

Precedence (later wins): optional config file → environment → Assist UI fields.

| Source | How |
|---|---|
| Environment | `COMPLIANCY_ASSIST_PROVIDER`, `COMPLIANCY_ASSIST_API_KEY`, `COMPLIANCY_ASSIST_ENDPOINT`, `COMPLIANCY_ASSIST_MODEL` |
| Config file | `COMPLIANCY_ASSIST_CONFIG=/path/to/assist.json`, or `~/.config/compliancy-checker/assist.json` |
| UI | Assist panel (session memory only; not written to disk by the app) |
| Mock / CI | Provider `mock`, or `COMPLIANCY_ASSIST_MOCK=1` — fixture responses, **no network** |

Example env for a real OpenAI-compatible run:

```bash
export COMPLIANCY_ASSIST_PROVIDER=openai
export COMPLIANCY_ASSIST_API_KEY=sk-...
export COMPLIANCY_ASSIST_MODEL=gpt-4o-mini
# optional:
# export COMPLIANCY_ASSIST_ENDPOINT=https://api.openai.com/v1
```

Copy [assist.example.json](assist.example.json) to a **private** path (e.g. `~/.config/compliancy-checker/assist.json`), fill in the key, and point `COMPLIANCY_ASSIST_CONFIG` at it if needed. Repo root `assist.json` / `assist.local.json` are gitignored.

Azure OpenAI: set provider `azure`, put the full deployment chat-completions URL (including `api-version`) in **Endpoint**, and use the Azure key as **API key**.

Local OpenAI-compatible servers: provider `local`, endpoint like `http://127.0.0.1:11434/v1`, model name as served.

### Tests without a real key

```bash
# Mock provider exercises Assist paths offline
COMPLIANCY_ASSIST_MOCK=1 cargo test -p compliancy-core
```

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
├── crates/pdf-core/             # PDF master inspect + PDF export
│   └── spike/                   # PDF fixtures, output, limits README
├── crates/compliancy-core/      # Pipeline + Assist LLM hook (CI-testable)
├── src-tauri/                   # Tauri desktop shell
├── assist.example.json          # Sample Assist config (no secrets)
└── README.md
```

## Licence

Proprietary — BDMTC. All rights reserved unless otherwise noted.
