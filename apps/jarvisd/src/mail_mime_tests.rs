//! The message builder: structure, file names, and what a stranger's headers may become.

use super::*;

fn decode(text: &str) -> Vec<u8> {
    let lines: String = text.split("\r\n").collect();
    let url_safe = lines.replace('+', "-").replace('/', "_");
    crate::google_tools::decode_base64url(url_safe.trim_end_matches('=')).unwrap_or_default()
}

#[test]
fn a_file_name_cannot_carry_a_path_a_quote_or_a_line_break() {
    assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
    assert_eq!(safe_file_name("C:\\Users\\me\\report.pdf"), "report.pdf");
    assert_eq!(
        safe_file_name("a\"b;c\r\nBcc: x@y.zz.pdf"),
        "a_b_c__Bcc_ x_y.zz.pdf"
    );
    assert_eq!(safe_file_name("..."), "attachment");
    assert_eq!(safe_file_name(""), "attachment");
    assert_eq!(safe_file_name("Überblick.pdf"), "Überblick.pdf");
    assert!(safe_file_name(&"x".repeat(500)).chars().count() <= 120);
}

#[test]
fn media_types_come_from_the_extension_and_unknown_is_opaque() {
    assert_eq!(content_type_for("Offer.PDF"), "application/pdf");
    assert_eq!(content_type_for("data.csv"), "text/csv");
    assert_eq!(content_type_for("noextension"), "application/octet-stream");
    assert_eq!(content_type_for("weird.xyz"), "application/octet-stream");
}

#[test]
fn programs_and_secrets_are_recognised_by_name() {
    for name in ["setup.EXE", "a.bat", "run.ps1", "x.js", "tool.sh"] {
        assert!(looks_executable(name), "{name}");
    }
    assert!(!looks_executable("report.pdf") && !looks_executable("exe"));
    for name in [
        ".env",
        ".env.local",
        "id_rsa",
        "server.pem",
        "api.key",
        "client.credential",
        "my-secrets.txt",
        "vault.kdbx",
    ] {
        assert!(looks_secret(name), "{name}");
    }
    assert!(!looks_secret("offer.pdf") && !looks_secret("keynote.pptx"));
}

#[test]
fn a_plain_message_is_one_base64_text_part() {
    let message = build("me@example.com", "Hi", "Hello there", None, &[], "BOUND");
    assert!(message.starts_with("To: me@example.com\r\nSubject: Hi\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8"));
    assert!(!message.contains("multipart") && !message.contains("BOUND"));
    let body = message.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert_eq!(decode(body.trim_end()), b"Hello there");
}

#[test]
fn attachments_make_a_multipart_message_that_carries_every_byte() {
    let bytes: Vec<u8> = (0..=255).collect();
    let files = [
        Attachment {
            name: "offer.pdf".to_owned(),
            bytes: bytes.clone(),
        },
        Attachment {
            name: "Überblick \"v2\".txt".to_owned(),
            bytes: b"hi".to_vec(),
        },
    ];
    let message = build(
        "me@example.com",
        "Offer",
        "See attached",
        None,
        &files,
        "B0UND",
    );
    assert!(message.contains("Content-Type: multipart/mixed; boundary=\"B0UND\""));
    assert_eq!(
        message.matches("--B0UND\r\n").count(),
        3,
        "the text and two files"
    );
    assert!(message.ends_with("--B0UND--\r\n"));
    assert!(message.contains("Content-Type: application/pdf; name=\"offer.pdf\""));
    assert!(message.contains("Content-Disposition: attachment; filename=\"offer.pdf\""));
    assert!(
        message.contains("filename*=UTF-8''%C3%9Cberblick%20%22v2%22.txt"),
        "{message}"
    );
    assert!(
        message.contains("filename=\"_berblick \"v2\".txt\"")
            || message.contains("name=\"_berblick"),
        "{message}"
    );
    let part = message.split("--B0UND\r\n").nth(2).unwrap_or_default();
    let data = part.split("\r\n\r\n").nth(1).unwrap_or_default();
    let data = data.split("\r\n--B0UND").next().unwrap_or_default();
    assert_eq!(decode(data), bytes, "every byte survives the encoding");
    for line in message.split("\r\n") {
        assert!(line.len() <= 998, "a line is within RFC 2822's limit");
    }
}

#[test]
fn reply_headers_keep_only_well_formed_identifiers() {
    let reply = reply_headers(
        "<abc.123@mail.example.com>",
        "<one@x.nl> junk <two@x.nl>\r\nBcc: evil@x.nl",
    )
    .unwrap_or_else(|| panic!("a usable reply"));
    assert_eq!(reply.in_reply_to, "<abc.123@mail.example.com>");
    assert_eq!(
        reply.references,
        "<one@x.nl> <two@x.nl> <abc.123@mail.example.com>"
    );
    assert!(!reply.references.contains("Bcc"));
    for bad in [
        "",
        "abc@x.nl",
        "<a b@x.nl>",
        "<a@x.nl>\r\nBcc: x@y.zz",
        "<@x.nl>",
        "<a@@x.nl>",
        "<a>",
    ] {
        assert_eq!(reply_headers(bad, ""), None, "{bad:?}");
    }
    let long = (0..100)
        .map(|i| format!("<m{i}@x.nl>"))
        .collect::<Vec<_>>()
        .join(" ");
    let cut = reply_headers("<last@x.nl>", &long).unwrap_or_else(|| panic!("usable"));
    assert!(cut.references.len() <= 800 && cut.references.ends_with("<last@x.nl>"));
}

#[test]
fn a_reply_message_names_its_thread_and_a_reply_subject_gets_one_re() {
    assert_eq!(reply_subject("Offer"), "Re: Offer");
    assert_eq!(reply_subject("RE: Offer"), "RE: Offer");
    assert_eq!(reply_subject("Of\r\nfer"), "Re: Offer");
    let reply = reply_headers("<a@x.nl>", "").unwrap_or_else(|| panic!("usable"));
    let message = build("me@example.com", "Re: Offer", "ok", Some(&reply), &[], "B");
    assert!(message.contains("\r\nIn-Reply-To: <a@x.nl>\r\nReferences: <a@x.nl>\r\nMIME-Version"));
}
