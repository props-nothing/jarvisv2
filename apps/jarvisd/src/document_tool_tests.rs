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
