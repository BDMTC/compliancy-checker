# `.docx` transform spike

Proves the hard part of Compliancy Checker v1: take a **source** Word document and a **master template**, write an **output** `.docx` whose factual body text comes from the source while fonts/colours/style definitions come from the template’s Open XML package (`styles.xml`, theme, etc.).

## What it does

1. Opens both packages as ZIP (Office Open XML).
2. Starts from the **template** package (styles, theme, section properties).
3. Replaces the template body (except trailing `w:sectPr`) with the source body paragraphs.
4. Writes `styled-output.docx`.

Style IDs on source paragraphs (`Heading1`, `Normal`, …) resolve against the template’s `styles.xml`.

## Prerequisites

- Rust toolchain (`rustc` / `cargo`) — see root [README](../../../README.md)

No Word/LibreOffice install is required to **run** the spike. Use either to **inspect** the output.

## Generate fixtures & run

From the repo root:

```bash
# Write sample-input.docx + master-template.docx
npm run spike:fixtures
# or: cargo run -p docx-core --bin docx-spike -- generate-fixtures

# Transform fixtures → crates/docx-core/spike/out/styled-output.docx
npm run spike:run
# or: cargo run -p docx-core --bin docx-spike -- run
```

Custom paths:

```bash
cargo run -p docx-core --bin docx-spike -- run \
  --source /path/to/source.docx \
  --template /path/to/master.docx \
  --output /path/to/out.docx
```

## Fixtures

| File | Role |
|---|---|
| `fixtures/sample-input.docx` | Times New Roman / black; holds customer address & inspection facts |
| `fixtures/master-template.docx` | Calibri; heading colour `#C45911`; body colour `#1F4E79` |
| `out/styled-output.docx` | Generated — facts from sample, styles from template |

## Unit test

```bash
cargo test -p docx-core
```

## Proved vs TODO

**Proved**

- Content preservation across style merge for paragraph text.
- Template `styles.xml` (fonts/colours) retained in the output package.
- Runnable without the Tauri GUI / without network.

**Still TODO**

- Headers/footers, numbering, complex runs (bold mid-sentence), tables, images.
- Style-ID remapping when source custom styles don’t exist on the template.
- Required-section / field validation from the master template.
- Structured context rule language; ISO clause checklist export.
- PDF ingest/export.
