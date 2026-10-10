---
integration: document-reading-crates
status: implemented
last_verified: 2026-10-10
owners: []
selected_spec_version: "lopdf 0.45.0; zip 9.0.1; roxmltree 0.21.1; ECMA-376 (Office Open XML) as the container format"
selected_sdk: "lopdf, zip, roxmltree (crates.io)"
---

# Reading PDF, Word, spreadsheet and slide files

## Scope

Extracting plain text from a PDF, `.docx`, `.xlsx` and `.pptx` file the owner already has inside a granted folder, for `jarvis.files.read_document` (`ADR-0158`). Out of scope: `.xls`, `.doc`, `.ppt` (the binary
formats), OCR of scanned pages, images, formulas' results beyond the cached value, and any writing of these formats.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| crate index and metadata | https://crates.io/api/v1/crates/lopdf (0.45.0, MIT, MSRV 1.88, updated 2026-09-08), `.../zip` (9.0.1, MIT, MSRV 1.88, updated 2026-10-10), `.../roxmltree` (0.21.1, MIT OR Apache-2.0, MSRV 1.60, updated 2025-10-12) | 2026-10-10 | versions, licences, features, dependencies |
| `llms.txt` | not found: none of the three publishes one | 2026-10-10 | |
| container format | ECMA-376 / ISO 29500 Office Open XML: a zip of XML parts; Word text is `w:t` runs in `word/document.xml`, spreadsheet text is `xl/sharedStrings.xml` plus `xl/worksheets/sheetN.xml` cells, slide text is `a:t` runs in `ppt/slides/slideN.xml` | not fetched (see Unresolved) | what the parser reads |
| security | the crates'' own documentation: `roxmltree` rejects a document type definition unless `allow_dtd` is set, and has `nodes_limit`; `zip` exposes entry sizes that a hostile archive can misstate | read in the crate source in the local registry | bounds |

## Verified Contract

- `lopdf::Document::load_mem(&[u8])`, `.is_encrypted()`, `.get_pages()` (page number to object id), `.extract_text(&[page_numbers])`. Default features (`chrono-clock`, `rayon`) are switched off. Its dependency set is large for a parser (AES and hash crates for encrypted
  files, `nom`, `encoding_rs`, `flate2`, `weezl`, `brotli-decompressor`, `rand`); that is its cost.
- `zip::ZipArchive::new(Cursor)`, `by_name`, `by_index_raw`, `len`; only the `deflate-flate2` feature is enabled, so none of bzip2, lzma, zstd or AES comes in.
- `roxmltree::Document::parse_with_options(text, ParsingOptions { allow_dtd: false, nodes_limit, .. })`.
- Not guaranteed by any of them: panic-freedom on hostile input, and bounded time. Both are JARVIS''s to provide.

## JARVIS Mapping

- A new crate `jarvis-documents` holds the three dependencies and one function, `extract(name, bytes)`, so no domain crate depends on a parser. The tool `jarvis.files.read_document` (in `jarvisd`) reads the bytes through a `WorkspaceRoots` handle, runs `extract` on a blocking
  thread with a 25 second timeout, and returns 3,500-character slices fenced as untrusted data.
- Bounds, all enforced by JARVIS: input at most 10 MB; any XML part at most 24 MB once unpacked and 48 MB per archive, counted as bytes are read and not by the entry''s own claimed size; at most 4,000 archive entries; 2,000,000 XML nodes; nesting depth 128; 300 PDF pages;
  30 sheets, 5,000 rows, 200 columns; output 400,000 characters.
- `catch_unwind` around every parse turns a parser panic into "could not be parsed".
- Effect: read-only, risk 0, no approval, scope `files.read`, like the other file reads. It can only look inside a folder the owner granted.

## Decisions

- Hand-written Office readers (`zip` plus `roxmltree`, about 300 lines) rather than `calamine` and `docx-rs`: fewer dependencies, and the text each needs is a handful of element names. A spreadsheet is read as the cached values, which is what a person sees.
- `lopdf` rather than `pdf-extract` (which wraps it and adds font and CMap crates): `lopdf` alone gives page text for the common case, and a document it cannot read says so.
- Scans are reported, not guessed at: no OCR.

## Rejected Alternatives

- `calamine` (spreadsheets of every era): large, and `.xls` and `.ods` are not a need yet.
- Shelling out to `pdftotext` or Office: a second program to install, and a process boundary the parsers do not need.
- Reading these formats inside `jarvis-tools`: it is a domain crate and should not carry parsers.

## Verification Plan

- Offline: documents are built in memory in the tests (a PDF through `lopdf` itself, Office files as zip archives): page order, runs, tabs, table cells, sparse spreadsheet cells, shared and inline strings, slide order; a zip that unpacks past the bound, an entity-expansion
  document, truncated and bit-flipped PDFs, and plain refusals.
- Cheapest test that would disprove the central assumption (that real-world Office files lay their text out as read here): open a `.docx`, `.xlsx` and `.pptx` produced by Microsoft Office and by LibreOffice. **Not run**; no such files were available to the build.

## Unresolved Questions

- Real files from Office and LibreOffice (blocks claiming "reads every .docx"). Known simplifications: sheets are found by position (`sheet1.xml` is the first listed), not through the workbook relationships; headers, footers, footnotes and comments of a Word file are not read; encrypted
  PDFs are refused rather than opened with an empty password.
- The duplicate crate versions this adds (`sha2`, `digest`, `block-buffer`, `crypto-common`, `cpufeatures`, `getrandom`, `hashbrown`, among others): `cargo deny` passes because `multiple-versions` is a warning in this repository, but the build is larger. Blocks nothing.

## Verification Log

- 2026-10-10: crates.io metadata read for the three crates; `cargo deny check` passes with them; tests in `crates/jarvis-documents`.
