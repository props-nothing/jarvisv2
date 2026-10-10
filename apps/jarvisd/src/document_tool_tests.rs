//! The document tool: it reads through a granted folder, pages long text, fences it, and refuses what it must.

use std::io::Write;

use super::*;

struct Folder(std::path::PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

fn docx(paragraphs: &[String]) -> Vec<u8> {
    let body = paragraphs.iter().fold(String::new(), |mut body, text| {
        body.push_str("<w:p><w:r><w:t>");
        body.push_str(text);
        body.push_str("</w:t></w:r></w:p>");
        body
    });
    let xml = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
    );
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    writer
        .write_all(xml.as_bytes())
        .unwrap_or_else(|error| panic!("{error}"));
    writer
        .finish()
        .unwrap_or_else(|error| panic!("{error}"))
        .into_inner()
}

fn tool_over_a_folder() -> (DocumentTool, Folder) {
    let folder = std::env::temp_dir().join(format!("jdt-{}", jarvis_core::scratch_tag()));
    std::fs::create_dir_all(folder.join("inbox")).unwrap_or_else(|error| panic!("{error}"));
    let roots = WorkspaceRoots::new([&folder]).unwrap_or_else(|error| panic!("{error}"));
    (DocumentTool::new(Arc::new(roots)), Folder(folder))
}

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

#[test]
fn the_tool_reads_without_asking_and_takes_no_absolute_path() {
    let definition = must(DocumentTool::definition());
    assert_eq!(definition.approval(), ApprovalPolicy::Auto);
    assert!(definition.effects().contains(ToolEffect::ReadOnly));
    assert_eq!(definition.risk().level(), 0);
    assert!(INPUT.contains(r#""additionalProperties": false"#));
}

#[tokio::test]
async fn a_long_document_is_read_in_slices_that_meet_exactly() {
    let (tool, folder) = tool_over_a_folder();
    let paragraphs: Vec<String> = (0..12)
        .map(|index| format!("Paragraph {index}: {}", "lorem ipsum ".repeat(60)))
        .collect();
    std::fs::write(folder.0.join("inbox").join("offer.docx"), docx(&paragraphs))
        .unwrap_or_else(|error| panic!("{error}"));

    let mut offset = 0_u64;
    let mut collected = String::new();
    let mut calls = 0;
    loop {
        let reply = must(
            tool.read(&json!({ "path": "inbox/offer.docx", "offset": offset }))
                .await,
        );
        assert_eq!(reply["kind"], "word");
        let content = reply["content"].as_str().unwrap_or_default();
        assert!(
            content.contains(jarvis_core::FENCE_OPEN),
            "a document is untrusted data: {content}"
        );
        collected.push_str(content);
        calls += 1;
        match reply["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset, "paging moves forward");
                offset = next;
            }
            None => break,
        }
        assert!(calls < 20, "paging ends");
    }
    assert!(
        calls >= 2,
        "a document this long needs more than one slice: {calls}"
    );
    assert!(
        collected.contains("Paragraph 0") && collected.contains("Paragraph 11"),
        "every part is reached"
    );

    // Past the end there is nothing and no next offset.
    let past = must(
        tool.read(&json!({ "path": "inbox/offer.docx", "offset": 1_000_000 }))
            .await,
    );
    assert!(past["content"].is_null() && past["next_offset"].is_null());
}

#[tokio::test]
async fn what_should_not_be_read_is_refused_in_words() {
    let (tool, folder) = tool_over_a_folder();
    std::fs::write(folder.0.join("notes.txt"), "plain text")
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(folder.0.join("broken.pdf"), b"%PDF-1.4 garbage")
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(folder.0.join("huge.docx"), vec![b'P'; 10 * 1024 * 1024 + 1])
        .unwrap_or_else(|error| panic!("{error}"));
    let outside = std::env::temp_dir().join("jdt-outside.docx");
    std::fs::write(&outside, docx(&["secret".to_owned()]))
        .unwrap_or_else(|error| panic!("{error}"));

    let cases = [
        ("../jdt-outside.docx", "no .."),
        (outside.to_str().unwrap_or_default(), "absolute"),
        ("C:\\Windows\\x.docx", "a drive"),
        ("missing.docx", "missing"),
        ("inbox", "a folder"),
        ("notes.txt", "plain text is the other tool"),
        ("broken.pdf", "damaged"),
        ("huge.docx", "larger than 10 MB"),
    ];
    for (path, why) in cases {
        let refused = tool.read(&json!({ "path": path })).await;
        assert!(refused.is_err(), "{why} must be refused");
    }
    let message = match tool.read(&json!({ "path": "notes.txt" })).await {
        Err(AdapterError::RefusedBeforeReaching { reason }) => reason,
        other => panic!("{other:?}"),
    };
    assert!(message.contains("PDF"), "{message}");
    let _ = std::fs::remove_file(outside);
}

/// A one-page PDF whose only content is the given picture stream, written out by hand with a correct cross-reference table.
fn scanned_pdf(picture: &[u8]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |out: &mut Vec<u8>, number: usize, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    object(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(&mut out, 2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    object(
        &mut out,
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /XObject << /Im1 4 0 R >> >> >>",
    );
    let mut image = format!(
        "<< /Type /XObject /Subtype /Image /Width 20 /Height 20 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
        picture.len()
    )
    .into_bytes();
    image.extend_from_slice(picture);
    image.extend_from_slice(b"\nendstream");
    object(&mut out, 4, &image);
    let content = b"q 612 0 0 792 0 0 cm /Im1 Do Q";
    let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content);
    stream.extend_from_slice(b"\nendstream");
    object(&mut out, 5, &stream);
    let table = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
    );
    out
}

#[tokio::test]
async fn a_picture_and_a_scanned_pdf_are_read_by_the_text_reader_and_marked_as_such() {
    let directory = crate::ocr::tests::scratch();
    let engine = crate::ocr::tests::echoing_engine(&directory);
    let (tool, folder) = tool_over_a_folder();
    let tool = tool.with_engine(Some(engine));
    let png = crate::ocr::tests::png(200, 100, b"\nline-from-the-picture\n");
    std::fs::write(folder.0.join("inbox").join("sign.png"), &png)
        .unwrap_or_else(|error| panic!("{error}"));
    let reply = must(
        tool.read(&json!({ "path": "inbox/sign.png", "language": "nld" }))
            .await,
    );
    assert_eq!(reply["kind"], "image");
    assert_eq!(reply["read_by"], "ocr");
    let content = reply["content"].as_str().unwrap_or_default();
    assert!(content.contains(jarvis_core::FENCE_OPEN), "{content}");
    assert!(content.contains("args:stdin stdout -l nld"), "{content}");
    assert!(content.contains("line-from-the-picture"), "{content}");

    // A PDF with no text, only a JPEG page, is read through the same reader, page by page.
    let jpeg = crate::ocr::tests::jpeg(300, 400);
    let pdf = scanned_pdf(&[jpeg.as_slice(), b"\nline-from-the-scan\n"].concat());
    std::fs::write(folder.0.join("inbox").join("scan.pdf"), pdf)
        .unwrap_or_else(|error| panic!("{error}"));
    let reply = must(tool.read(&json!({ "path": "inbox/scan.pdf" })).await);
    assert_eq!(reply["kind"], "scanned_pdf");
    assert_eq!(reply["read_by"], "ocr");
    let content = reply["content"].as_str().unwrap_or_default();
    assert!(content.contains("--- page 1 ---"), "{content}");
    assert!(content.contains("line-from-the-scan"), "{content}");
    jarvis_core::remove_scratch_dir(&directory);
}

#[tokio::test]
async fn without_the_engine_the_answer_says_how_to_get_it_and_a_bad_picture_never_reaches_it() {
    let (tool, folder) = tool_over_a_folder();
    let missing = tool.with_engine(None);
    std::fs::write(folder.0.join("a.png"), crate::ocr::tests::png(10, 10, b""))
        .unwrap_or_else(|error| panic!("{error}"));
    let message = match missing.read(&json!({ "path": "a.png" })).await {
        Err(AdapterError::RefusedBeforeReaching { reason }) => reason,
        other => panic!("{other:?}"),
    };
    assert!(
        message.contains("tesseract") && message.contains("install"),
        "{message}"
    );

    // Even with an engine present, a picture that would unpack to gigabytes and a language that reads as an option are refused.
    let directory = crate::ocr::tests::scratch();
    let engine = crate::ocr::tests::echoing_engine(&directory);
    let (tool, folder) = tool_over_a_folder();
    let tool = tool.with_engine(Some(engine));
    std::fs::write(
        folder.0.join("bomb.png"),
        crate::ocr::tests::png(50_000, 50_000, b""),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        folder.0.join("ok.png"),
        crate::ocr::tests::png(10, 10, b"x"),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(tool.read(&json!({ "path": "bomb.png" })).await.is_err());
    assert!(
        tool.read(&json!({ "path": "ok.png", "language": "--tessdata-dir" }))
            .await
            .is_err()
    );
    jarvis_core::remove_scratch_dir(&directory);
}
