# PDF master template + PDF export (CC-11)

Extends Compliancy Checker so a **PDF** can be chosen as the master template and/or as the **save/output** format alongside `.docx`.

## What it does

1. **Inspect PDF master** — extract usable cues: `BaseFont` names, page `MediaBox` size, best-effort RGB samples from content streams. Same trait-picker shape as Word masters.
2. **PDF → Word bridge** — synthesise a styles-only `.docx` from those cues, then run the existing Open XML merge.
3. **Export PDF** — regenerate a new PDF from extracted document text + selected cues (mapped to standard Type1 fonts).

## Limits

- Not a binary/pixel-perfect restyle of an existing PDF.
- Embedded custom fonts are **not** subset-copied; names map to Helvetica / Times / Courier (Word: Arial / Times New Roman / Courier New).
- Layout chrome (headers/footers, images, multi-column, absolute frames) is not applied.
- Colours are sampled from content operators when present — not a full brand-pack extraction.
- Scanned / image-only PDFs yield little or no usable cues (**no OCR** in v1).
- Tables / inline images from source `.docx` flatten to plain text in PDF export.
- Single-page PDF export in v1 (long docs truncate rather than invent complex pagination).

## CLI

```bash
# Write fixtures/master-template.pdf
npm run spike:pdf-fixtures
# or: cargo run -p pdf-core --bin pdf-spike -- generate-fixtures

# Inspect cues
npm run spike:pdf-inspect
# or: cargo run -p pdf-core --bin pdf-spike -- inspect-template \
#       --template crates/pdf-core/spike/fixtures/master-template.pdf

# Export sample-input.docx → styled-output.pdf (optional --template)
npm run spike:pdf-export
```

## Tests

```bash
cargo test -p pdf-core
cargo test -p compliancy-core
```
