//! The extractors, over documents built in memory, and the refusals that keep a hostile one from hurting the caller.

use std::io::Write;

use lopdf::{Document, Object, Stream, dictionary};
use zip::write::SimpleFileOptions;

use super::*;

/// A zip archive holding the given `(name, contents)` entries.
fn archive_of(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, contents) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap_or_else(|error| panic!("{error}"));
        writer
            .write_all(contents.as_bytes())
            .unwrap_or_else(|error| panic!("{error}"));
    }
    writer
        .finish()
        .unwrap_or_else(|error| panic!("{error}"))
        .into_inner()
}

fn docx(body: &str) -> Vec<u8> {
    let xml = format!(
        r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
    );
    archive_of(&[("word/document.xml", &xml)])
}

fn pdf_of(pages: &[&str]) -> Vec<u8> {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font = document.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources = document.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let mut kids = Vec::new();
    for text in pages {
        let content = format!("BT /F1 12 Tf 72 700 Td ({text}) Tj ET");
        let stream = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page = document.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => stream, "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(Object::Reference(page));
    }
    let count = i64::try_from(kids.len()).unwrap_or(0);
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .unwrap_or_else(|error| panic!("{error}"));
    bytes
}

#[test]
fn a_word_document_reads_paragraphs_runs_tabs_and_table_cells() {
    let bytes = docx(
        r"<w:p><w:r><w:t>Offer </w:t></w:r><w:r><w:t>for Acme</w:t></w:r></w:p>
          <w:p><w:r><w:t>Price</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>EUR 4,000</w:t></w:r></w:p>
          <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Cost</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
    );
    let found = extract("offer.DOCX", &bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(found.kind, Kind::Word);
    assert!(
        found.text.starts_with("Offer for Acme\nPrice\tEUR 4,000\n"),
        "{}",
        found.text
    );
    assert!(
        found.text.contains("Item\t") && found.text.contains("Cost"),
        "{}",
        found.text
    );
    assert!(!found.truncated);
}

#[test]
fn a_spreadsheet_reads_every_sheet_with_cells_where_they_sit() {
    let workbook = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheets><sheet name="Leads" sheetId="1"/><sheet name="Totals" sheetId="2"/></sheets></workbook>"#;
    let strings = r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Company</t></si><si><r><t>Ac</t></r><r><t>me</t></r></si><si><t>City</t></si></sst>"#;
    let sheet1 = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
        <row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="s"><v>2</v></c></row>
        <row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2"><v>42</v></c><c r="C2" t="b"><v>1</v></c><c r="D2" t="inlineStr"><is><t>tab	here</t></is></c></row>
        <row r="3"><c r="A3"/></row></sheetData></worksheet>"#;
    let sheet2 = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="B1"><v>7</v></c></row></sheetData></worksheet>"#;
    let bytes = archive_of(&[
        ("xl/workbook.xml", workbook),
        ("xl/sharedStrings.xml", strings),
        ("xl/worksheets/sheet1.xml", sheet1),
        ("xl/worksheets/sheet2.xml", sheet2),
    ]);
    let found = extract("leads.xlsx", &bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((found.kind, found.parts), (Kind::Spreadsheet, 2));
    assert_eq!(
        found.text,
        "--- sheet: Leads ---\nCompany\t\tCity\nAcme\t42\tTRUE\ttab here\n--- sheet: Totals ---\n\t7"
    );
}

#[test]
fn slides_are_read_in_numeric_order() {
    let slide = |text: &str| {
        format!(
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sld>"#
        )
    };
    let (ten, two, one) = (slide("Ten"), slide("Two"), slide("One"));
    let bytes = archive_of(&[
        ("ppt/slides/slide10.xml", &ten),
        ("ppt/slides/slide2.xml", &two),
        ("ppt/slides/slide1.xml", &one),
    ]);
    let found = extract("deck.pptx", &bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(found.parts, 3);
    assert_eq!(
        found.text,
        "--- slide 1 ---\nOne\n--- slide 2 ---\nTwo\n--- slide 10 ---\nTen"
    );
}

#[test]
fn a_pdf_reads_its_pages_in_order_and_a_damaged_one_is_refused_not_fatal() {
    let bytes = pdf_of(&["Invoice 42", "Total due EUR 900"]);
    let found = extract("invoice.pdf", &bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((found.kind, found.parts), (Kind::Pdf, 2));
    assert!(
        found.text.contains("--- page 1 ---") && found.text.contains("Invoice 42"),
        "{}",
        found.text
    );
    assert!(
        found.text.contains("--- page 2 ---") && found.text.contains("Total due EUR 900"),
        "{}",
        found.text
    );

    // Truncated and mutated copies of a real PDF never panic.
    for cut in [10, 40, bytes.len() / 2, bytes.len() - 5] {
        let _ = extract("x.pdf", &bytes[..cut]);
    }
    for index in (0..bytes.len()).step_by(7) {
        let mut mutated = bytes.clone();
        mutated[index] ^= 0xFF;
        let _ = extract("x.pdf", &mutated);
    }
    assert_eq!(
        extract("x.pdf", b"%PDF-1.4 not really"),
        Err(DocumentError::Malformed)
    );
}

#[test]
fn what_is_not_a_document_this_reads_is_refused() {
    assert_eq!(
        extract("notes.txt", b"hello"),
        Err(DocumentError::Unsupported)
    );
    assert_eq!(
        extract("a.zip", &archive_of(&[("a.txt", "x")])),
        Err(DocumentError::Unsupported)
    );
    assert_eq!(
        extract("a.docx", b"not a zip at all"),
        Err(DocumentError::Unsupported)
    );
    assert_eq!(
        extract("a.docx", &archive_of(&[("other.xml", "<a/>")])),
        Err(DocumentError::Malformed)
    );
    assert_eq!(
        extract("a.docx", b"PK\x03\x04garbage"),
        Err(DocumentError::Malformed)
    );
}

#[test]
fn an_xml_document_type_definition_is_refused_so_no_entity_can_expand() {
    let bomb = r#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;">]><w:document xmlns:w="w"><w:p><w:t>&b;</w:t></w:p></w:document>"#;
    let bytes = archive_of(&[("word/document.xml", bomb)]);
    assert_eq!(extract("a.docx", &bytes), Err(DocumentError::Malformed));
}

#[test]
fn an_archive_that_unpacks_past_the_bound_is_refused_whatever_its_size_on_disk() {
    // About 27 MB of XML that compresses to a few kilobytes: small on disk, large once opened.
    let huge = format!(
        "<w:document xmlns:w=\"w\">{}</w:document>",
        "<w:p/>".repeat(4_500_000)
    );
    let bytes = archive_of(&[("word/document.xml", &huge)]);
    assert!(
        bytes.len() < 200_000,
        "the archive is small: {}",
        bytes.len()
    );
    assert_eq!(extract("a.docx", &bytes), Err(DocumentError::TooLarge));
    assert_eq!(
        extract("a.pdf", &vec![b'%'; MAX_INPUT_BYTES + 1]),
        Err(DocumentError::TooLarge)
    );
}

#[test]
fn a_long_document_is_cut_at_the_bound_and_says_so() {
    let paragraph = format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", "word ".repeat(2000));
    let bytes = docx(&paragraph.repeat(100));
    let found = extract("long.docx", &bytes).unwrap_or_else(|error| panic!("{error}"));
    assert!(found.truncated);
    assert!(found.text.chars().count() <= MAX_OUTPUT_CHARS);
}
