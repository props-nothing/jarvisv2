# ADR-0160: Web pages are read in parts, PDFs on the web are read, and Word notes are read

Status: Accepted
Date: 2026-10-10

## Context

`jarvis.web.fetch` returned the first 4,000 characters of a page and nothing more, and answered a PDF with its type and size only. A research run that follows a link to a long article, a price list or a report could not read past the opening. The document reader (`ADR-0158`) skipped the text Word keeps outside the body:
headers, footers, footnotes and endnotes. The owner confirmed the reader on real Office, LibreOffice and PDF files.

## Decision

1. **Paging.** `jarvis.web.fetch` takes `offset` (characters into the page text) and answers with `total_chars` and `next_offset` (null at the end). The page is fetched again for each part, which is a harmless repeat of a read, and each part is fenced as untrusted data like the first. `truncated` now also means "more remains". The size cap on what is read from the network (256 KB) is unchanged.
2. **PDFs.** A response of type `application/pdf` is read (up to 5 MB, parsed on a blocking thread under a 15 second limit) with `jarvis-documents`, the same bounded and panic-safe parser the file reader uses, and its text is paged like any page. `kind` is `pdf` and `bytes_read` is the PDF's size. A PDF cut at the size cap is not parsed (half a PDF has no structure) and one that cannot be read is a failure that says why. Egress rules (no private
   networks, redirect checks) apply as before: a PDF comes through the same guarded fetch.
3. **Word notes.** `.docx` reading appends running headers and footers (each distinct text once, so the same header on every page is said once), footnotes and endnotes, with their numbers; the separators Word writes between the body and its notes are skipped.

## A stop no longer waits for an open stream

Found while restarting the daemon for this change: `jarvis stop` made the daemon begin its graceful shutdown, and it then stayed alive, unreachable, for minutes until killed. A graceful HTTP shutdown waits for every open connection, and the console keeps a live stream open to watch runs, which never finishes by itself.
The transport now waits at most 8 seconds for connections to finish and then closes them (`drain`), logging that it did. A test runs a real server with a stream that never ends and checks that the drain gives up on it, and that a server with nothing open finishes at once.

## Consequences

A research run can read a long article to the end and follow a link to a PDF. The cost is repeated fetches while paging, and a PDF is a heavier thing to take from a stranger, which the bounds and the existing egress guard are for. Not built: PDF paging by byte range (the whole PDF is fetched each time), caching of fetched pages between parts, other document types over the web, and OCR.