# ADR-0158: JARVIS reads PDF, Word, spreadsheet and slide files

Status: Accepted
Date: 2026-10-10

## Context

`jarvis.files.read` reads text files and refuses anything else. The files people send a prospecting or project run are mostly not text: an offer as a PDF, a price list as a spreadsheet, a brief as a Word file. With the email attachment tools (`ADR-0157`) a run could save one into a granted folder and
then not open it.

## Decision

1. **A tool**, `jarvis.files.read_document`, reads `.pdf`, `.docx`, `.xlsx` and `.pptx` inside a granted folder, through the same confinement handles as the file tools. Read-only, risk 0, no approval (the other file reads are the same). It answers 3,500 characters at a time with the kind, the number of
   pages or sheets, the total length and `next_offset`, and the text is **fenced as untrusted data**: the author of a document is not the owner.
2. **A crate**, `jarvis-documents`, holds the parsers (`lopdf`, `zip`, `roxmltree`) and one function. No domain crate depends on a parser. Research and the dependency cost are in `docs/research/integrations/document-reading-crates.md`.
3. **A document is hostile input**, and the crate says so in code: input at most 10 MB; XML parts bounded by what they unpack to as they are read, not by what the archive claims; an entry-count limit; no XML document type definition (no entity expansion); node and depth limits; every parse inside `catch_unwind`; the tool abandons a parse that takes longer than 25 seconds. A password-protected PDF
   is reported, not opened; a scan (no text) is reported as such. Tests include a zip that is tiny on disk and huge when opened, an entity-expansion document, and truncated and bit-flipped PDFs.
4. Spreadsheets come out as tab-separated rows per sheet, with cells where they sit; PDFs by page.

## Consequences

A run can read what it receives. The cost is a larger dependency tree (`lopdf` brings AES, hashing and decoding crates, which duplicate some versions already present; `cargo deny` passes). Not built: OCR, the old binary Office formats, headers and footnotes of Word files, and a check against files produced by real Office and LibreOffice (the
tests build their documents in memory). `P9-069` no longer lists PDF and spreadsheet reading.
