//! Plain text out of the documents people send: PDF, Word (`.docx`), spreadsheets (`.xlsx`) and slides (`.pptx`) (`ADR-0158`).
//!
//! A pure library: bytes in, text out, no files, no network. It exists so that the parsers (and their dependencies) stay out of the domain
//! crates, and so that the one rule that matters about them is stated once: **a document is hostile input.** Every entry point bounds
//! what it will read, refuses what it cannot bound, and cannot take the caller down: a parser that panics is caught and reported as
//! "malformed", an archive that unpacks to far more than it claims is refused, XML with a document type definition is refused (no entity
//! expansion), and a PDF that asks for a password is reported rather than guessed at.

use std::fmt::Write as _;
use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};

use roxmltree::{Document, Node, ParsingOptions};
use thiserror::Error;

/// The largest input accepted, in bytes.
pub const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;
/// The most text one extraction produces, in characters. Longer documents are cut and say so.
pub const MAX_OUTPUT_CHARS: usize = 400_000;
/// The most pages of a PDF read.
const MAX_PDF_PAGES: usize = 300;
/// The most pages of a scanned PDF whose pictures are handed on to be read as text.
pub const MAX_SCAN_PAGES: usize = 10;
/// The largest single embedded picture handed on, in bytes.
const MAX_SCAN_IMAGE_BYTES: usize = 6 * 1024 * 1024;
/// The most bytes any one XML part of an archive may unpack to.
const MAX_XML_BYTES: u64 = 24 * 1024 * 1024;
/// The most bytes read out of one archive in total.
const MAX_ARCHIVE_BYTES: u64 = 48 * 1024 * 1024;
/// The most entries an archive may hold.
const MAX_ENTRIES: usize = 4000;
/// The most XML nodes one part may have.
const MAX_NODES: u32 = 2_000_000;
/// Walking a document stops a little past the output bound, so the cut is noticed and reported rather than silently reached.
const FLOW_LIMIT: usize = MAX_OUTPUT_CHARS + 50_000;
/// The deepest element nesting walked.
const MAX_DEPTH: usize = 128;
/// The most sheets, rows per sheet and cells per row read from a spreadsheet.
const MAX_SHEETS: usize = 30;
const MAX_ROWS: usize = 5000;
const MAX_COLUMNS: usize = 200;

/// What kind of document it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A PDF: `parts` counts pages.
    Pdf,
    /// A Word document.
    Word,
    /// A spreadsheet: `parts` counts sheets.
    Spreadsheet,
    /// A slide deck: `parts` counts slides.
    Slides,
}

impl Kind {
    /// A name for the tool's answer.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Word => "word",
            Self::Spreadsheet => "spreadsheet",
            Self::Slides => "slides",
        }
    }
}

/// The text of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// What it was.
    pub kind: Kind,
    /// Its text, at most [`MAX_OUTPUT_CHARS`] characters.
    pub text: String,
    /// Pages, sheets or slides read.
    pub parts: usize,
    /// Whether the text was cut at a bound.
    pub truncated: bool,
}

/// Why a document could not be read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DocumentError {
    /// The name or the contents are not a kind this reads.
    #[error("only PDF, .docx, .xlsx and .pptx files can be read")]
    Unsupported,
    /// The file is larger than [`MAX_INPUT_BYTES`], or unpacks to more than the bounds allow.
    #[error("the document is too large to read safely")]
    TooLarge,
    /// The document needs a password.
    #[error("the document is protected with a password")]
    Encrypted,
    /// The document is damaged, holds no text (a scan is pictures), or is not what its name says.
    #[error("the document could not be parsed, or holds no text to read")]
    Malformed,
    /// The PDF is a scan, but none of its pictures is in a form that can be handed on (only JPEG-encoded pages are).
    #[error(
        "the PDF is made of pictures, but not in a form that can be read (only JPEG-encoded scans are supported)"
    )]
    ScanUnsupported,
}

/// The pictures of a scanned PDF: the JPEG of each page, with the page number, in page order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// `(page number, JPEG bytes)`, at most [`MAX_SCAN_PAGES`] of them.
    pub pages: Vec<(u32, Vec<u8>)>,
    /// Whether the PDF has more pages than were taken.
    pub cut: bool,
}

/// Takes the JPEG picture of each page out of a scanned PDF, so that it can be read as text by something that reads pictures.
///
/// Only a picture stored as a bare JPEG (`DCTDecode` and nothing else) is taken: that stream *is* a JPEG file. Pictures in any other
/// encoding are not decoded here; a PDF holding only those is reported as [`DocumentError::ScanUnsupported`]. A page with several
/// pictures contributes the largest. Never panics.
///
/// # Errors
///
/// [`DocumentError`] for an oversized, protected or damaged PDF, or one with no usable picture.
pub fn pdf_scan(bytes: &[u8]) -> Result<Scan, DocumentError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(DocumentError::TooLarge);
    }
    catch_unwind(AssertUnwindSafe(|| scan_pages(bytes))).unwrap_or(Err(DocumentError::Malformed))
}

fn scan_pages(bytes: &[u8]) -> Result<Scan, DocumentError> {
    if !bytes.starts_with(b"%PDF") {
        return Err(DocumentError::Unsupported);
    }
    let document = lopdf::Document::load_mem(bytes).map_err(|_| DocumentError::Malformed)?;
    if document.is_encrypted() {
        return Err(DocumentError::Encrypted);
    }
    let pages = document.get_pages();
    let mut taken = Vec::new();
    for (number, id) in pages.iter().take(MAX_PDF_PAGES) {
        let Ok(images) = document.get_page_images(*id) else {
            continue;
        };
        let best = images
            .iter()
            .filter(|image| {
                image.filters.as_deref() == Some(&["DCTDecode".to_owned()][..])
                    && image.content.starts_with(&[0xFF, 0xD8, 0xFF])
                    && image.content.len() <= MAX_SCAN_IMAGE_BYTES
            })
            .max_by_key(|image| image.content.len());
        if let Some(image) = best {
            if taken.len() == MAX_SCAN_PAGES {
                return Ok(Scan {
                    pages: taken,
                    cut: true,
                });
            }
            taken.push((*number, image.content.to_vec()));
        }
    }
    if taken.is_empty() {
        return Err(DocumentError::ScanUnsupported);
    }
    Ok(Scan {
        pages: taken,
        cut: pages.len() > MAX_PDF_PAGES,
    })
}

/// Reads the text of a document, choosing the parser from its contents (and the name when the contents do not say).
///
/// # Errors
///
/// [`DocumentError`] for an unsupported, oversized, protected or damaged document. Never panics.
pub fn extract(name: &str, bytes: &[u8]) -> Result<Extracted, DocumentError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(DocumentError::TooLarge);
    }
    let extension = std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    // A parser written for friendly input is not trusted to be panic-free on hostile input.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if bytes.starts_with(b"%PDF") {
            pdf(bytes)
        } else if bytes.starts_with(b"PK") {
            match extension.as_str() {
                "docx" => word(bytes),
                "xlsx" => spreadsheet(bytes),
                "pptx" => slides(bytes),
                _ => Err(DocumentError::Unsupported),
            }
        } else {
            Err(DocumentError::Unsupported)
        }
    }));
    outcome.unwrap_or(Err(DocumentError::Malformed))
}

/// Cuts text to [`MAX_OUTPUT_CHARS`], reporting whether it did.
fn bounded(mut text: String) -> (String, bool) {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return (text, false);
    }
    let cut = text
        .char_indices()
        .nth(MAX_OUTPUT_CHARS)
        .map_or(text.len(), |(index, _)| index);
    text.truncate(cut);
    (text, true)
}

fn pdf(bytes: &[u8]) -> Result<Extracted, DocumentError> {
    let document = lopdf::Document::load_mem(bytes).map_err(|_| DocumentError::Malformed)?;
    if document.is_encrypted() {
        return Err(DocumentError::Encrypted);
    }
    let pages = document.get_pages();
    let mut text = String::new();
    let mut read = 0;
    let mut cut = pages.len() > MAX_PDF_PAGES;
    let mut any = false;
    for number in pages.keys().take(MAX_PDF_PAGES) {
        // A page that cannot be read is skipped and said so in the text rather than failing the whole document.
        match document.extract_text(&[*number]) {
            Ok(page) if !page.trim().is_empty() => {
                any = true;
                let _ = writeln!(text, "--- page {number} ---\n{}", page.trim());
            }
            _ => {
                let _ = writeln!(text, "--- page {number}: no text ---");
            }
        }
        read += 1;
        if text.chars().count() > MAX_OUTPUT_CHARS {
            cut = true;
            break;
        }
    }
    // A scan is pictures of text: there is none to extract, and the caller should be told that rather than shown an empty answer.
    if !any {
        return Err(DocumentError::Malformed);
    }
    let (text, truncated) = bounded(text);
    Ok(Extracted {
        kind: Kind::Pdf,
        text,
        parts: read,
        truncated: truncated || cut,
    })
}

fn archive(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>, DocumentError> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| DocumentError::Malformed)?;
    if archive.len() > MAX_ENTRIES {
        return Err(DocumentError::TooLarge);
    }
    Ok(archive)
}

/// Reads one part of an archive as text, refusing one that unpacks past the bound and counting every byte against the archive's budget.
fn part(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    budget: &mut u64,
) -> Result<Option<String>, DocumentError> {
    let Ok(entry) = archive.by_name(name) else {
        return Ok(None);
    };
    let mut text = String::new();
    // One byte past the bound tells "exactly the bound" from "more", whatever the entry's own header claims.
    let limit = MAX_XML_BYTES.min(*budget);
    let read = entry
        .take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|_| DocumentError::Malformed)?;
    if read as u64 > limit {
        return Err(DocumentError::TooLarge);
    }
    *budget -= read as u64;
    Ok(Some(text))
}

fn parse(text: &str) -> Result<Document<'_>, DocumentError> {
    // No document type definition is accepted (the default), which is what rules out entity expansion.
    Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
            ..ParsingOptions::default()
        },
    )
    .map_err(|_| DocumentError::Malformed)
}

fn local<'a>(node: Node<'a, '_>) -> &'a str {
    node.tag_name().name()
}

/// Appends the text under an element of a Word or slide document: runs in order, a paragraph per line, a tab between table cells.
fn flow(node: Node<'_, '_>, depth: usize, out: &mut String) {
    if depth > MAX_DEPTH || out.chars().count() > FLOW_LIMIT {
        return;
    }
    match local(node) {
        "t" => out.push_str(node.text().unwrap_or_default()),
        "tab" => out.push('\t'),
        "br" | "cr" => out.push('\n'),
        // A table cell is one tab-separated field on its row, however many paragraphs it holds, and a row is one line.
        "tc" | "tr" => {
            let mut inner = String::new();
            for child in node.children().filter(Node::is_element) {
                flow(child, depth + 1, &mut inner);
            }
            out.push_str(inner.trim_end_matches(['\n', '\t']));
            out.push(if local(node) == "tc" { '\t' } else { '\n' });
        }
        name => {
            for child in node.children().filter(Node::is_element) {
                flow(child, depth + 1, out);
            }
            if name == "p" {
                out.push('\n');
            }
        }
    }
}

fn word(bytes: &[u8]) -> Result<Extracted, DocumentError> {
    let mut archive = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_BYTES;
    let xml =
        part(&mut archive, "word/document.xml", &mut budget)?.ok_or(DocumentError::Malformed)?;
    let document = parse(&xml)?;
    let mut text = String::new();
    flow(document.root_element(), 0, &mut text);
    word_extras(&mut archive, &mut budget, &mut text)?;
    let stopped = text.chars().count() > FLOW_LIMIT;
    let (text, truncated) = bounded(tidy(&text));
    Ok(Extracted {
        kind: Kind::Word,
        text,
        parts: 1,
        truncated: truncated || stopped,
    })
}

/// Appends what a Word file keeps outside its body: running headers and footers (each distinct text once), footnotes and endnotes.
fn word_extras(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    budget: &mut u64,
    text: &mut String,
) -> Result<(), DocumentError> {
    let mut names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|name| name.starts_with("word/header") || name.starts_with("word/footer"))
        .filter(|name| {
            std::path::Path::new(name.as_ref())
                .extension()
                .is_some_and(|extension| extension == "xml")
        })
        .map(std::borrow::Cow::into_owned)
        .collect();
    names.sort();
    let mut running: Vec<String> = Vec::new();
    for name in names.iter().take(12) {
        let Some(xml) = part(archive, name, budget)? else {
            continue;
        };
        let mut found = String::new();
        flow(parse(&xml)?.root_element(), 0, &mut found);
        let found = tidy(&found);
        if !found.is_empty() && !running.contains(&found) {
            running.push(found);
        }
    }
    if !running.is_empty() {
        let _ = writeln!(
            text,
            "\n--- headers and footers ---\n{}",
            running.join("\n")
        );
    }
    for (file, heading, element) in [
        ("word/footnotes.xml", "footnotes", "footnote"),
        ("word/endnotes.xml", "endnotes", "endnote"),
    ] {
        let Some(xml) = part(archive, file, budget)? else {
            continue;
        };
        let document = parse(&xml)?;
        let mut notes = String::new();
        for note in document
            .root_element()
            .children()
            .filter(|node| node.is_element() && local(*node) == element)
            // The separators Word writes between the body and its notes are not notes.
            .filter(|node| node.attribute("type").is_none_or(|kind| kind == "normal"))
        {
            let mut body = String::new();
            flow(note, 0, &mut body);
            let body = tidy(&body);
            if !body.is_empty() {
                let id = note.attribute("id").unwrap_or("?");
                let _ = writeln!(notes, "[{id}] {body}");
            }
        }
        if !notes.is_empty() {
            let _ = writeln!(text, "\n--- {heading} ---\n{notes}");
        }
    }
    Ok(())
}

fn slides(bytes: &[u8]) -> Result<Extracted, DocumentError> {
    let mut archive = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_BYTES;
    let mut numbers: Vec<usize> = (0..archive.len())
        .filter_map(|index| {
            let entry = archive.by_index_raw(index).ok()?;
            let name = entry.name().ok()?.into_owned();
            name.strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse()
                .ok()
        })
        .collect();
    numbers.sort_unstable();
    if numbers.is_empty() {
        return Err(DocumentError::Malformed);
    }
    let mut text = String::new();
    for number in &numbers {
        let xml = part(
            &mut archive,
            &format!("ppt/slides/slide{number}.xml"),
            &mut budget,
        )?
        .ok_or(DocumentError::Malformed)?;
        let document = parse(&xml)?;
        let _ = writeln!(text, "--- slide {number} ---");
        flow(document.root_element(), 0, &mut text);
        if text.chars().count() > FLOW_LIMIT {
            break;
        }
    }
    let stopped = text.chars().count() > FLOW_LIMIT;
    let (text, truncated) = bounded(tidy(&text));
    Ok(Extracted {
        kind: Kind::Slides,
        text,
        parts: numbers.len(),
        truncated: truncated || stopped,
    })
}

/// Trims trailing spaces on each line and collapses runs of blank lines to one.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_owned()
}

/// The text of every `t` under `node`, joined: one shared string, which may be split into runs.
fn joined_text(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(|descendant| descendant.is_element() && local(*descendant) == "t")
        .filter_map(|descendant| descendant.text())
        .collect()
}

/// A spreadsheet column letter (`A`, `AB`) as a zero-based index.
fn column_index(reference: &str) -> Option<usize> {
    let letters: Vec<u8> = reference
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .collect();
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let total = letters.iter().fold(0_usize, |total, letter| {
        total * 26 + usize::from(letter.to_ascii_uppercase() - b'A' + 1)
    });
    Some(total - 1)
}

/// One cell's shown value: a shared string, an inline string, a boolean, or the number as written.
fn cell_text(cell: Node<'_, '_>, shared: &[String]) -> String {
    let value = cell
        .children()
        .find(|node| node.is_element() && local(*node) == "v")
        .and_then(|node| node.text())
        .unwrap_or_default();
    let shown = match cell.attribute("t") {
        Some("s") => value
            .parse::<usize>()
            .ok()
            .and_then(|at| shared.get(at).cloned())
            .unwrap_or_default(),
        Some("inlineStr") => joined_text(cell),
        Some("b") => (if value == "1" { "TRUE" } else { "FALSE" }).to_owned(),
        _ => value.to_owned(),
    };
    shown.replace(['\t', '\n', '\r'], " ")
}

/// One sheet's rows as tab-separated lines. Returns whether a bound cut it.
fn sheet_rows(sheet: &Document<'_>, shared: &[String], text: &mut String) -> bool {
    let mut rows = 0;
    for row in sheet
        .descendants()
        .filter(|node| node.is_element() && local(*node) == "row")
    {
        rows += 1;
        if rows > MAX_ROWS {
            return true;
        }
        let mut cells: Vec<String> = Vec::new();
        for cell in row
            .children()
            .filter(|node| node.is_element() && local(*node) == "c")
        {
            let column = cell
                .attribute("r")
                .and_then(column_index)
                .unwrap_or(cells.len())
                .min(MAX_COLUMNS);
            while cells.len() < column {
                cells.push(String::new());
            }
            cells.push(cell_text(cell, shared));
        }
        while cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        if !cells.is_empty() {
            text.push_str(&cells.join("\t"));
            text.push('\n');
        }
        if text.chars().count() > MAX_OUTPUT_CHARS {
            return true;
        }
    }
    false
}

fn spreadsheet(bytes: &[u8]) -> Result<Extracted, DocumentError> {
    let mut archive = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_BYTES;
    let workbook =
        part(&mut archive, "xl/workbook.xml", &mut budget)?.ok_or(DocumentError::Malformed)?;
    let workbook = parse(&workbook)?;
    let shared_xml = part(&mut archive, "xl/sharedStrings.xml", &mut budget)?;
    let shared_doc = shared_xml.as_deref().map(parse).transpose()?;
    let shared: Vec<String> = shared_doc
        .as_ref()
        .map(|document| {
            document
                .root_element()
                .children()
                .filter(|node| node.is_element() && local(*node) == "si")
                .map(joined_text)
                .collect()
        })
        .unwrap_or_default();
    // Sheets are named in the workbook and found by position: `sheet1.xml` is the first one listed, which is how the writers this
    // has met lay them out; one that does not is read as far as the numbering allows.
    let names: Vec<String> = workbook
        .descendants()
        .filter(|node| node.is_element() && local(*node) == "sheet")
        .filter_map(|node| node.attribute("name").map(str::to_owned))
        .take(MAX_SHEETS)
        .collect();
    if names.is_empty() {
        return Err(DocumentError::Malformed);
    }
    let mut text = String::new();
    let mut read = 0;
    let mut cut = false;
    for (index, name) in names.iter().enumerate() {
        let Some(xml) = part(
            &mut archive,
            &format!("xl/worksheets/sheet{}.xml", index + 1),
            &mut budget,
        )?
        else {
            continue;
        };
        let sheet = parse(&xml)?;
        let _ = writeln!(text, "--- sheet: {name} ---");
        read += 1;
        cut = sheet_rows(&sheet, &shared, &mut text);
        if cut {
            break;
        }
    }
    let (text, truncated) = bounded(text.trim().to_owned());
    Ok(Extracted {
        kind: Kind::Spreadsheet,
        text,
        parts: read,
        truncated: truncated || cut,
    })
}

#[cfg(test)]
mod tests;
