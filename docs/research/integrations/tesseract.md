---
integration: tesseract
status: implemented-minimal
last_verified: 2026-10-10
owners: []
selected_spec_version: "tesseract command line as documented in the `main` man page (tesseract.1.asc) and the tessdoc site; no version pinned, the program is the owner's own install"
selected_sdk: null
---

# Tesseract (text out of pictures)

## Scope

One operation: read the text of one picture (PNG, JPEG or BMP) by running the owner's installed `tesseract` program, so that the document reader (`ADR-0158`) can answer for a photographed page, a screenshot, or a PDF that is only pictures (`ADR-0162`). Out of scope: bundling or linking an engine, layout or table recognition, handwriting, PDF output, TIFF and GIF input, training, and any engine other than `tesseract`.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | not found: the project publishes no `llms.txt`; docs discovered from https://tesseract-ocr.github.io/tessdoc/ | 2026-10-10 | discovery |
| API/specification | https://raw.githubusercontent.com/tesseract-ocr/tesseract/main/doc/tesseract.1.asc (man page, `main`) | 2026-10-10 | normative contract |
| command-line guide | https://tesseract-ocr.github.io/tessdoc/Command-Line-Usage.html | 2026-10-10 | usage |
| installation | https://tesseract-ocr.github.io/tessdoc/Installation.html | 2026-10-10 | how the owner gets it, where it lands |
| licence | https://raw.githubusercontent.com/tesseract-ocr/tesseract/main/LICENSE (Apache 2.0) | 2026-10-10 | distribution |
| OpenAPI/AsyncAPI/schema | not applicable: a command line program | 2026-10-10 | |
| changelog/release notes | not read: only the oldest part of the command line is used (input, output base, `-l`) | 2026-10-10 | |

## Verified Contract

### Operations And Transport

- Synopsis: `tesseract FILE OUTPUTBASE [OPTIONS]... [CONFIGFILE]...`. `FILE` is an image "readable by Leptonica"; "if `FILE` is `stdin` or `-` then the standard input is used". `OUTPUTBASE` "if `stdout` or `-` then the standard output is used"; the default output is plain text.
- `-l LANG` selects the language, "for example `-l eng+deu+fra`"; options `-l` and `--psm` "must occur before any CONFIGFILE". With no `-l` the default is English.
- JARVIS runs exactly `tesseract stdin stdout [-l LANG]`, no config file, no shell, with the picture on standard input and the text read from standard output.

### Authentication And Authorization

None: a local program. The trust question is what it is given and what it may do, below.

### Limits And Failure Semantics

- The man page states no time or size limit. JARVIS adds its own: a header check (at most 40 megapixels and 20,000 pixels a side, from the PNG, JPEG or BMP header, before the engine sees the file), 45 s per picture and 100 s per document, 2 MB of output, the process killed on any of them. At most 10 pages of a scanned PDF are read.
- A non-zero exit (an unknown language is the usual cause) is reported as a failure.

### Data And Compliance

- Nothing leaves the machine: the picture and the text stay local.
- Installation: the engine and each language's `traineddata` are separate parts (Installation page). Windows: the installer from UB Mannheim (https://github.com/UB-Mannheim/tesseract/wiki), "probably `C:\Program Files\Tesseract-OCR`"; macOS: Homebrew or MacPorts; Linux: the `tesseract-ocr` package, with `tesseract-ocr-<langcode>` per language. The Windows installer may not add itself to `PATH`; JARVIS also looks in `Program Files\Tesseract-OCR`.
- Licence: Apache 2.0. JARVIS only runs the program the owner installed; nothing is bundled or linked.

### Versions And Deprecations

No version is pinned. The command line used is the old, stable subset. Tesseract 3.02, 3.05, 4 and 5 are mentioned by the Windows installers; only the documented `main` man page was read.

## JARVIS Mapping

- Not a separate tool: `jarvis.files.read_document` (read-only, risk 0, no approval) gains pictures and scanned PDFs, with an optional `language`. The answer says `read_by: "ocr"` so the model knows to expect small mistakes, and is fenced as untrusted data like every document text.
- Scanned PDFs: `jarvis-documents::pdf_scan` takes the JPEG of each page (a stream filtered by `DCTDecode` alone *is* a JPEG file) with lopdf; other encodings are not decoded and are reported as unsupported.
- Missing engine: the tool says so and how to install it; nothing else changes.

## Decisions

- **Standard input, not a temporary file:** no path to race or inject; the picture never touches disk outside the granted folder.
- **Found by JARVIS, not by the operating system's search:** only absolute `PATH` entries and fixed install directories, never the current directory, which on Windows a plain `Command::new("tesseract")` can search first.
- **Environment cleared** down to `PATH`, `SystemRoot`, `TESSDATA_PREFIX`, `TEMP`, `TMP`, `HOME`.
- **Language validated** (one to four codes of letters, digits and `_`, joined by `+`, starting with a letter) so it can never read as an option.

## Rejected Alternatives

- Linking Leptonica/Tesseract through a crate: a large native build and a parser of hostile pictures inside the daemon's own process; a child process contains a crash and can be killed.
- A cloud OCR service: sends the owner's documents to a third party.
- Decoding Flate/CCITT/JPX PDF images in-process: more hostile-input code for a case the JPEG path does not cover; documented as unsupported instead.

## Verification Plan

- Contract tests with a stand-in program (done): arguments, standard input, time limit, header limits, language validation, absolute-only search, scanned PDF to pages.
- **Not done: a run against a real `tesseract`.** No engine is installed on the development machine, so nothing here is verified against real recognition output, real language data, or a real scanned PDF. Falsified by: installing the engine, reading a photographed page and a JPEG-scanned PDF, and comparing.

## Unresolved

- Whether the Windows installer's default location matches the search (from the docs: probably yes). Blocks nothing; the engine is also found on `PATH`.
- Memory use of the engine on a large legal picture (under the 40-megapixel cap) is not measured; the process is not memory-limited.