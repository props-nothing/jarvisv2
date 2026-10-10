//! Building the email Gmail's `raw` field wants: plain text, optionally as a reply, optionally with attachments (`ADR-0157`).
//!
//! Gmail takes "RFC 2822 formatted MIME message, encoded as base64URL" (`docs/research/integrations/google.md`, "Sending with
//! attachments"). A message with files is `multipart/mixed`: the text first, then one base64 part per file. A reply is threaded by the
//! `Subject` matching and `In-Reply-To` and `References` following RFC 2822.
//!
//! Everything that came from outside is made safe here, in one place, because a header or a boundary is where a line break turns data
//! into structure: a file name is reduced to printable characters that cannot close a quoted value, and the reply headers (copied from
//! a message a stranger wrote) are kept only if they are well-formed message identifiers.

use std::fmt::Write as _;

use crate::google_tools::encode_base64;

/// A file to attach: its name as the recipient sees it, and its bytes.
pub(crate) struct Attachment {
    /// The file name, already reduced by [`safe_file_name`].
    pub(crate) name: String,
    /// The bytes to send.
    pub(crate) bytes: Vec<u8>,
}

/// The headers that make a message a reply in its thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplyHeaders {
    /// The `Message-ID` of the message replied to.
    pub(crate) in_reply_to: String,
    /// The thread's message identifiers, oldest first, ending with `in_reply_to`.
    pub(crate) references: String,
}

/// The longest file name sent.
const MAX_NAME_CHARS: usize = 120;
/// The longest `References` value kept (RFC 2822 asks for lines of at most 998 characters; a long thread is cut from the front).
const MAX_REFERENCES_CHARS: usize = 800;

/// A file name safe to put in a header: the last path component, printable ASCII and a few harmless marks, never empty.
pub(crate) fn safe_file_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || " ._-()+,".contains(character) {
                character
            } else if character.is_alphanumeric() {
                // A letter from another script is kept: `filename*` carries it, and the plain `filename` falls back to `_`.
                character
            } else {
                '_'
            }
        })
        .take(MAX_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "attachment".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The media type for a file name, from its extension; anything unknown is opaque bytes.
pub(crate) fn content_type_for(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ics" => "text/calendar",
        _ => "application/octet-stream",
    }
}

/// Whether a name is one a person's mail client treats as a program. These are never saved from an email.
pub(crate) fn looks_executable(name: &str) -> bool {
    const PROGRAMS: [&str; 16] = [
        "exe", "bat", "cmd", "com", "scr", "msi", "ps1", "vbs", "js", "jar", "sh", "lnk", "dll",
        "app", "apk", "hta",
    ];
    name.rsplit_once('.')
        .is_some_and(|(_, extension)| PROGRAMS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Whether a name is one that usually holds a secret: sending it by mail is refused outright.
pub(crate) fn looks_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower.rsplit(['/', '\\']).next().unwrap_or_default();
    stem.starts_with(".env")
        || stem.starts_with("id_rsa")
        || stem.starts_with("id_ed25519")
        || stem.contains("credential")
        || stem.contains("secret")
        || std::path::Path::new(stem)
            .extension()
            .is_some_and(|extension| {
                matches!(extension.to_str(), Some("pem" | "key" | "pfx" | "kdbx"))
            })
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"._-".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn wrapped_base64(bytes: &[u8]) -> String {
    let encoded = encode_base64(bytes, false, true);
    let lines: Vec<&str> = encoded
        .as_bytes()
        .chunks(76)
        .filter_map(|line| std::str::from_utf8(line).ok())
        .collect();
    lines.join("\r\n")
}

/// One message identifier as RFC 2822 writes it: `<` a run of safe characters, `@`, more, `>`.
fn is_message_id(token: &str) -> bool {
    let Some(inner) = token
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
    else {
        return false;
    };
    inner.len() <= 200
        && inner.matches('@').count() == 1
        && !inner.starts_with('@')
        && !inner.ends_with('@')
        && inner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._%+-@=$!#&'*/?^`{|}~".contains(&byte))
}

/// Builds the reply headers from what the original message carried, or `None` when it carried no usable identifier.
///
/// The values were written by whoever sent the original, so each token is checked and the rest dropped: a stray line break or angle
/// bracket must not become a new header.
pub(crate) fn reply_headers(message_id: &str, references: &str) -> Option<ReplyHeaders> {
    let id = message_id.trim();
    if !is_message_id(id) {
        return None;
    }
    let mut chain: Vec<&str> = references
        .split_whitespace()
        .filter(|token| is_message_id(token))
        .collect();
    if chain.last() != Some(&id) {
        chain.push(id);
    }
    // The newest identifiers matter most, so a long thread is cut from the front.
    let mut kept: Vec<&str> = Vec::new();
    let mut length = 0;
    for token in chain.iter().rev() {
        length += token.len() + 1;
        if length > MAX_REFERENCES_CHARS {
            break;
        }
        kept.push(token);
    }
    kept.reverse();
    Some(ReplyHeaders {
        in_reply_to: id.to_owned(),
        references: kept.join(" "),
    })
}

/// A subject for a reply: the original's, with one `Re:` in front.
pub(crate) fn reply_subject(original: &str) -> String {
    let flat: String = original
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let trimmed = flat.trim();
    if trimmed.len() >= 3 && trimmed[..3].eq_ignore_ascii_case("re:") {
        trimmed.to_owned()
    } else {
        format!("Re: {trimmed}")
    }
}

/// The message, headers then body, ready to be encoded for `raw`. `boundary` must be a fresh random token.
pub(crate) fn build(
    to: &str,
    subject: &str,
    body: &str,
    reply: Option<&ReplyHeaders>,
    attachments: &[Attachment],
    boundary: &str,
) -> String {
    let subject = if subject.is_ascii() {
        subject.to_owned()
    } else {
        format!(
            "=?UTF-8?B?{}?=",
            encode_base64(subject.as_bytes(), false, true)
        )
    };
    let mut message = format!("To: {to}\r\nSubject: {subject}\r\n");
    if let Some(reply) = reply {
        let _ = write!(
            message,
            "In-Reply-To: {}\r\nReferences: {}\r\n",
            reply.in_reply_to, reply.references
        );
    }
    message.push_str("MIME-Version: 1.0\r\n");
    let text = wrapped_base64(body.as_bytes());
    if attachments.is_empty() {
        let _ = write!(
            message,
            "Content-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{text}\r\n"
        );
        return message;
    }
    let _ = write!(
        message,
        "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{text}\r\n"
    );
    for attachment in attachments {
        let ascii: String = attachment
            .name
            .chars()
            .map(|character| if character.is_ascii() { character } else { '_' })
            .collect();
        let _ = write!(
            message,
            "--{boundary}\r\nContent-Type: {kind}; name=\"{ascii}\"\r\nContent-Disposition: attachment; filename=\"{ascii}\"; filename*=UTF-8''{pct}\r\nContent-Transfer-Encoding: base64\r\n\r\n{data}\r\n",
            kind = content_type_for(&attachment.name),
            pct = percent_encode(&attachment.name),
            data = wrapped_base64(&attachment.bytes),
        );
    }
    let _ = write!(message, "--{boundary}--\r\n");
    message
}

#[cfg(test)]
#[path = "mail_mime_tests.rs"]
mod tests;
