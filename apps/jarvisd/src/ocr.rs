//! Text out of a picture, by handing it to the `tesseract` program the owner has installed (`ADR-0162`).
//!
//! JARVIS does not bundle an OCR engine. If `tesseract` is on the machine it is used; if not, the caller is told how to get it, and nothing
//! else changes. A picture is hostile input like any other document, so this is careful in the same ways:
//!
//! - the program is found by this module, in the directories of `PATH` that are absolute and in a few fixed install places, never in the
//!   current directory, and it is run with fixed arguments and no shell;
//! - the picture goes in on standard input, so there is no temporary file to race and no path to inject;
//! - the picture's header is read first and one that would unpack to a huge number of pixels is refused before the engine sees it;
//! - the run has a time limit and the answer a size limit, the program is killed when either is passed, and its environment is cleared down
//!   to what it needs to find its language data.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// The most pixels a picture may have (width times height). A phone photo of a page is about twelve million.
const MAX_PIXELS: u64 = 40_000_000;
/// The longest side a picture may have.
const MAX_SIDE: u64 = 20_000;
/// The most bytes read back from the engine.
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
/// How long one picture may take.
pub const PICTURE_SECONDS: u64 = 45;
/// The pictures of a PDF share one budget, so a long scan answers rather than running into the tool's own time limit.
pub const TOTAL_SECONDS: u64 = 100;

/// Why a picture could not be read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OcrError {
    /// No `tesseract` could be found.
    #[error(
        "reading text from pictures needs the free `tesseract` program, which is not installed (or not found). Install it \
         (Windows: the installer from https://github.com/UB-Mannheim/tesseract/wiki; macOS: `brew install tesseract`; Linux: the \
         `tesseract-ocr` package; each language other than English needs its own data), then ask again"
    )]
    Missing,
    /// The bytes are not a picture this reads, or are too large in pixels.
    #[error(
        "that is not a PNG, JPEG or BMP picture this can read, or it is too large (over 40 megapixels)"
    )]
    UnreadablePicture,
    /// The language names are not in the form `eng` or `eng+nld`.
    #[error("the language must be tesseract language codes such as `eng` or `nld+eng`")]
    BadLanguage,
    /// The engine did not finish in time.
    #[error("the text reader took too long and was stopped")]
    TimedOut,
    /// The engine could not be started or ended with an error (an unknown language is the usual cause).
    #[error("the text reader failed; if a language was given, check it is installed for tesseract")]
    Failed,
}

/// Finds the `tesseract` program: in the absolute directories of `PATH`, then in the places the usual installers use.
#[must_use]
pub fn locate() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut directories: Vec<PathBuf> = std::env::split_paths(&path).collect();
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(variable) {
            directories.push(PathBuf::from(base).join("Tesseract-OCR"));
        }
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        directories.push(PathBuf::from(base).join("Programs").join("Tesseract-OCR"));
    }
    for fixed in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
        directories.push(PathBuf::from(fixed));
    }
    locate_in(&directories)
}

/// The first `tesseract` (or `tesseract.exe`) that is a file in one of the given directories. A relative directory is skipped: whatever
/// the current directory holds must never be run.
#[must_use]
pub fn locate_in(directories: &[PathBuf]) -> Option<PathBuf> {
    directories
        .iter()
        .filter(|directory| directory.is_absolute())
        .flat_map(|directory| {
            ["tesseract", "tesseract.exe"]
                .into_iter()
                .map(|name| directory.join(name))
        })
        .find(|candidate| candidate.is_file())
}

/// The size in pixels of a PNG, JPEG or BMP, read from its header. `None` for anything else, and for a header that does not hold one.
#[must_use]
pub fn picture_size(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let header = bytes.get(16..24)?;
        return Some((
            u64::from(u32::from_be_bytes(header[0..4].try_into().ok()?)),
            u64::from(u32::from_be_bytes(header[4..8].try_into().ok()?)),
        ));
    }
    if bytes.starts_with(b"BM") {
        let header = bytes.get(18..26)?;
        let width = i32::from_le_bytes(header[0..4].try_into().ok()?);
        let height = i32::from_le_bytes(header[4..8].try_into().ok()?);
        return Some((
            u64::from(width.unsigned_abs()),
            u64::from(height.unsigned_abs()),
        ));
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return jpeg_size(bytes);
    }
    None
}

/// Walks a JPEG's segments to its start-of-frame marker, which holds the size.
fn jpeg_size(bytes: &[u8]) -> Option<(u64, u64)> {
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        // The frame markers, without the ones that share the range but are not frames (DHT, JPG, DAC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let frame = bytes.get(at + 5..at + 9)?;
            let height = u64::from(u16::from_be_bytes([frame[0], frame[1]]));
            let width = u64::from(u16::from_be_bytes([frame[2], frame[3]]));
            return Some((width, height));
        }
        if length < 2 {
            return None;
        }
        at += 2 + length;
    }
    None
}

/// Whether the bytes are a picture this reads and a sane size.
#[must_use]
pub fn is_readable_picture(bytes: &[u8]) -> bool {
    picture_size(bytes).is_some_and(|(width, height)| {
        width > 0
            && height > 0
            && width <= MAX_SIDE
            && height <= MAX_SIDE
            && width * height <= MAX_PIXELS
    })
}

/// Whether the bytes start like a picture this reads (before any size check).
#[must_use]
pub fn looks_like_picture(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"BM")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// Whether the language is one to three tesseract codes joined by `+`, nothing that could be read as an option.
#[must_use]
pub fn valid_language(language: &str) -> bool {
    let parts: Vec<&str> = language.split('+').collect();
    (1..=4).contains(&parts.len())
        && parts.iter().all(|code| {
            (2..=12).contains(&code.len())
                && code.starts_with(|first: char| first.is_ascii_alphabetic())
                && code
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

/// Reads the text of one picture.
///
/// # Errors
///
/// [`OcrError`] when the picture or language is not acceptable, the engine cannot be run, or it takes too long.
pub async fn recognise(
    executable: &Path,
    picture: &[u8],
    language: Option<&str>,
    limit: Duration,
) -> Result<String, OcrError> {
    if !is_readable_picture(picture) {
        return Err(OcrError::UnreadablePicture);
    }
    if language.is_some_and(|language| !valid_language(language)) {
        return Err(OcrError::BadLanguage);
    }
    let mut command = Command::new(executable);
    command.arg("stdin").arg("stdout");
    if let Some(language) = language {
        command.arg("-l").arg(language);
    }
    command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // The engine finds its language files through these and the operating system needs the others to start a program at all.
    for name in [
        "PATH",
        "SystemRoot",
        "TESSDATA_PREFIX",
        "TEMP",
        "TMP",
        "HOME",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|_| OcrError::Failed)?;
    let mut input = child.stdin.take().ok_or(OcrError::Failed)?;
    let mut output = child.stdout.take().ok_or(OcrError::Failed)?;
    let picture = picture.to_vec();
    let run = async move {
        let write = async move {
            // A program that stops reading early closes the pipe; that is its answer, not an error of ours.
            let _ = input.write_all(&picture).await;
            let _ = input.shutdown().await;
        };
        let read = async {
            let mut text = Vec::new();
            let _ = (&mut output)
                .take(MAX_OUTPUT_BYTES as u64)
                .read_to_end(&mut text)
                .await;
            text
        };
        let (text, ()) = tokio::join!(read, write);
        let status = child.wait().await;
        (text, status)
    };
    let (text, status) = tokio::time::timeout(limit, run)
        .await
        .map_err(|_| OcrError::TimedOut)?;
    if !status.is_ok_and(|status| status.success()) {
        return Err(OcrError::Failed);
    }
    Ok(clean(&String::from_utf8_lossy(&text)))
}

/// Drops control characters and trailing blanks, so the text is plain lines.
fn clean(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.chars()
                .filter(|character| !character.is_control() || *character == '\t')
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
#[path = "ocr_tests.rs"]
pub(crate) mod tests;
