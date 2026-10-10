//! The Google tools' dealings with files: attaching them to mail, and saving what arrives (`ADR-0157`).
//!
//! Both go through the folders the owner granted (`WorkspaceRoots`), which are held as open handles that cannot be left: a path that
//! climbs out, or follows a link out, fails exactly as it does for the file tools. What leaves the machine is limited by the approval
//! the owner gives to every send (the card shows each file's path) and by refusals that do not depend on anyone's judgement: a file
//! whose name says it holds a secret is never attached, and a program that arrives by mail is never saved.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::google_tools::{Failure, GoogleTool, decode_base64url};
use crate::mail_mime::{Attachment, looks_executable, looks_secret, safe_file_name};

/// The most files one email carries.
pub(crate) const MAX_ATTACHMENTS: usize = 5;
/// The largest single file sent or saved, in bytes.
pub(crate) const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// The most bytes of files one email carries. Gmail's own limit for a message is larger; this is the conservative figure the research
/// record could verify nothing above (`docs/research/integrations/google.md`).
pub(crate) const MAX_TOTAL_BYTES: u64 = 10 * 1024 * 1024;
/// The folder a saved attachment goes to when none is named.
pub(crate) const DEFAULT_SAVE_FOLDER: &str = "email-attachments";

/// One attachment found in a message.
pub(crate) struct FoundAttachment {
    pub(crate) name: String,
    pub(crate) mime: String,
    pub(crate) size: u64,
    pub(crate) attachment_id: Option<String>,
    pub(crate) inline_data: Option<String>,
}

/// Every attachment in a message payload, in order. Parts with no file name are the message's text, not attachments.
pub(crate) fn find_attachments(payload: &Value) -> Vec<FoundAttachment> {
    fn walk(part: &Value, depth: usize, found: &mut Vec<FoundAttachment>) {
        if depth > 6 || found.len() >= 25 {
            return;
        }
        let name = part
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = part.get("body");
        let attachment_id = body
            .and_then(|body| body.get("attachmentId"))
            .and_then(Value::as_str);
        let inline_data = body
            .and_then(|body| body.get("data"))
            .and_then(Value::as_str);
        if !name.is_empty() && (attachment_id.is_some() || inline_data.is_some()) {
            found.push(FoundAttachment {
                name: name.to_owned(),
                mime: part
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream")
                    .to_owned(),
                size: body
                    .and_then(|body| body.get("size"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                attachment_id: attachment_id.map(str::to_owned),
                inline_data: inline_data.map(str::to_owned),
            });
        }
        if let Some(parts) = part.get("parts").and_then(Value::as_array) {
            for child in parts {
                walk(child, depth + 1, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(payload, 0, &mut found);
    found
}

/// A relative path made only of ordinary names: no drive, no leading slash, no `..`, nothing that is not a plain component.
pub(crate) fn plain_relative(text: &str) -> Option<PathBuf> {
    if text.is_empty() || text.chars().count() > 260 || text.chars().any(char::is_control) {
        return None;
    }
    let path = Path::new(text);
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then(|| path.to_path_buf())
}

fn bad(detail: &str) -> Failure {
    Failure::new("bad_arguments", detail)
}

impl GoogleTool {
    /// Gives the tool the folders the owner granted, so it can attach files from them and save attachments into them.
    #[must_use]
    pub fn with_workspace(mut self, roots: std::sync::Arc<jarvis_tools::WorkspaceRoots>) -> Self {
        self.workspace = Some(roots);
        self
    }

    /// Reads the files an email is to carry, each from a granted folder and within the size limits.
    pub(crate) fn load_attachments(&self, names: &[&str]) -> Result<Vec<Attachment>, Failure> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let Some(roots) = &self.workspace else {
            return Err(Failure::new(
                "not_permitted",
                "attachments come from a folder the owner granted, and none is: the owner grants one in Settings, Files",
            ));
        };
        if names.len() > MAX_ATTACHMENTS {
            return Err(bad("an email carries at most 5 attachments"));
        }
        let mut files = Vec::with_capacity(names.len());
        let mut total = 0_u64;
        for text in names {
            let relative = plain_relative(text).ok_or_else(|| {
                bad("an attachment is a path inside a granted folder, such as reports/offer.pdf (no drive, no .., not absolute)")
            })?;
            let name = safe_file_name(text);
            // Judged on the name as given: sanitising would turn `.env` into `env`.
            if looks_secret(text) || looks_secret(&name) {
                return Err(Failure::new(
                    "not_permitted",
                    "a file whose name says it holds a secret (a key, a credential, an .env file) is never attached",
                ));
            }
            let (is_directory, size) = roots
                .entry_kind(&relative)
                .map_err(|_| bad("an attachment was not found in the granted folders"))?;
            if is_directory {
                return Err(bad("an attachment must be a file, not a folder"));
            }
            if size > MAX_FILE_BYTES {
                return Err(bad("an attachment is larger than 5 MB"));
            }
            total += size;
            if total > MAX_TOTAL_BYTES {
                return Err(bad("the attachments together are larger than 10 MB"));
            }
            let (file, _) = roots
                .open_file(&relative)
                .map_err(|_| bad("an attachment could not be opened"))?;
            let mut bytes = Vec::new();
            file.take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| bad("an attachment could not be read"))?;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(bad("an attachment is larger than 5 MB"));
            }
            files.push(Attachment { name, bytes });
        }
        Ok(files)
    }

    /// The attachment's bytes: the part's own data when it is small enough to be inline, else fetched by its attachment id.
    async fn attachment_bytes(
        &self,
        id: &str,
        chosen: &FoundAttachment,
    ) -> Result<Vec<u8>, Failure> {
        let data = match (&chosen.inline_data, &chosen.attachment_id) {
            (Some(data), _) => data.clone(),
            (None, Some(attachment_id)) => {
                if attachment_id.is_empty()
                    || attachment_id.len() > 1024
                    || !attachment_id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                {
                    return Err(Failure::new(
                        "google_failed",
                        "Google sent an attachment id this tool cannot use",
                    ));
                }
                let fetched = self
                    .get(
                        &format!(
                            "{}/users/me/messages/{id}/attachments/{attachment_id}",
                            self.account.gmail_base()
                        ),
                        &[],
                    )
                    .await?;
                fetched
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            }
            (None, None) => return Err(bad("that attachment has no content")),
        };
        let bytes = decode_base64url(&data).ok_or_else(|| {
            Failure::new(
                "google_failed",
                "Google sent attachment data that could not be decoded",
            )
        })?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(bad("that attachment is larger than 5 MB"));
        }
        Ok(bytes)
    }
    /// Saves one attachment of a message into a granted folder and returns a sentence saying where. Never replaces a file.
    pub(crate) async fn save_attachment(&self, arguments: &Value) -> Result<String, Failure> {
        let Some(roots) = self.workspace.clone() else {
            return Err(Failure::new(
                "not_permitted",
                "saving needs a folder the owner granted, and none is: the owner grants one in Settings, Files",
            ));
        };
        let id = arguments
            .get("message_id")
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 64
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            })
            .ok_or_else(|| bad("a message id is required"))?;
        let folder = arguments
            .get("folder")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_SAVE_FOLDER);
        let folder = plain_relative(folder).ok_or_else(|| {
            bad("the folder is a path inside a granted folder (no drive, no .., not absolute)")
        })?;
        let message = self.get_message(id, "full").await?;
        let found = find_attachments(message.get("payload").unwrap_or(&Value::Null));
        if found.is_empty() {
            return Err(bad("that message has no attachments"));
        }
        let wanted = match arguments.get("attachment") {
            Some(Value::Number(number)) => number.as_u64().map(|n| n.to_string()),
            Some(Value::String(text)) => Some(text.trim().to_owned()),
            _ => None,
        }
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            bad("name the attachment: its number or its file name, as jarvis.gmail.read lists them")
        })?;
        let chosen = match wanted.parse::<usize>() {
            Ok(number) => found.get(number.wrapping_sub(1)),
            Err(_) => found
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(&wanted)),
        }
        .ok_or_else(|| bad("no attachment of that message has that number or name"))?;
        let name = safe_file_name(&chosen.name);
        if looks_executable(&name) {
            return Err(Failure::new(
                "not_permitted",
                "a program that arrives by email is never saved: the owner can open it from their own mail",
            ));
        }
        if chosen.size > MAX_FILE_BYTES {
            return Err(bad("that attachment is larger than 5 MB"));
        }
        let bytes = self.attachment_bytes(id, chosen).await?;
        let (kept, label) = write_unique(&roots, &folder, &name, &bytes)?;
        Ok(format!(
            "saved {} bytes to {} (in the granted folder {label}); it is untrusted: open it only if the owner asks",
            bytes.len(),
            kept.display()
        ))
    }
}

/// Writes a new file, picking `name (2).ext`, `name (3).ext` when the name is taken: a saved attachment never replaces a file.
fn write_unique(
    roots: &jarvis_tools::WorkspaceRoots,
    folder: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<(PathBuf, String), Failure> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (name.to_owned(), String::new()),
    };
    for attempt in 1..=20 {
        let candidate = if attempt == 1 {
            name.to_owned()
        } else {
            format!("{stem} ({attempt}){extension}")
        };
        let path = folder.join(&candidate);
        match roots.write_file(&path, bytes, false) {
            Ok((_, label)) => return Ok((path, label.display().to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(Failure::new(
                    "not_permitted",
                    "the file could not be written there (the folder must be inside a granted folder)",
                ));
            }
        }
    }
    Err(bad("twenty files of that name already exist there"))
}
