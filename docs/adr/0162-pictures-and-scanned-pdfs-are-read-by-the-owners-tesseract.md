# ADR-0162: Pictures and scanned PDFs are read by the owner's tesseract

Status: Accepted
Date: 2026-10-10

## Context

The document reader (`ADR-0158`) says "a scan is pictures of text" and stops. A photographed invoice, a screenshot of an order, or a PDF made by a scanner is the document people actually send, and it was unreadable.

## Decision

1. **`jarvis.files.read_document` also reads a picture** (PNG, JPEG, BMP) **and a PDF that has no text but has page pictures**, by running the owner's installed `tesseract` (`docs/research/integrations/tesseract.md`). JARVIS bundles and links no engine; if none is found the answer says how to install it and nothing else changes. An optional `language` (`nld`, `nld+eng`) is validated and passed with `-l`.
2. **Containment.** The program is found only in absolute `PATH` entries and fixed install directories, never the current directory. It runs with fixed arguments, no shell, a cleared environment, the picture on standard input (no temporary file), 45 s per picture and 100 s per document, 2 MB of output, killed on any of them. A picture's header is read first and one over 40 megapixels or 20,000 pixels a side is refused before the engine sees it (a small file can unpack to gigabytes). The tool stays read-only, risk 0, unprompted: it reads only from the granted folders and writes nothing.
3. **Scanned PDFs** are handled by `jarvis-documents::pdf_scan`: the JPEG of each page (a `DCTDecode`-only stream is a JPEG file; the magic bytes are checked), at most 10 pages, 6 MB each. Pages in other encodings are not decoded; the answer says only JPEG scans are supported.
4. **Honest output.** `read_by: "ocr"` tells the model the text may contain mistakes; the text is fenced as untrusted data like every document text; pages with no result are marked. The last OCR result is kept in memory so reading a long scan in slices does not run the engine again for each slice.

## Consequences

Scans and photos become readable once the owner installs tesseract (plus the language data for non-English text). Not built: TIFF and GIF, PDF images that are not JPEG, layout analysis, a memory limit on the engine process (the pixel cap is the guard), and, most importantly, **a run against a real tesseract**: no engine was installed on the development machine, so the tests use a stand-in program that records its arguments and input.