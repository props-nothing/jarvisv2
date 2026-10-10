use std::path::PathBuf;
use std::time::Duration;

use super::*;

pub(crate) fn png(width: u32, height: u32, tail: &[u8]) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&13_u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(tail);
    bytes
}

pub(crate) fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46];
    bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
    bytes
}

pub(crate) fn scratch() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("jocr-{}", jarvis_core::scratch_tag()));
    std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
    directory
}

/// A stand-in for `tesseract` that prints the arguments it was given and then whatever arrived on standard input, so a test can see both.
pub(crate) fn echoing_engine(directory: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = directory.join("fake.cmd");
        std::fs::write(&path, "@echo off\r\necho args:%*\r\nfindstr \"^\"\r\n")
            .unwrap_or_else(|error| panic!("{error}"));
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("fake.sh");
        std::fs::write(&path, "#!/bin/sh\nprintf 'args:%s\\n' \"$*\"\ncat\n")
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{error}"));
        path
    }
}

fn sleeping_engine(directory: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = directory.join("slow.cmd");
        std::fs::write(&path, "@echo off\r\nping -n 8 127.0.0.1 >nul\r\n")
            .unwrap_or_else(|error| panic!("{error}"));
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("slow.sh");
        std::fs::write(&path, "#!/bin/sh\nsleep 8\n").unwrap_or_else(|error| panic!("{error}"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{error}"));
        path
    }
}

#[test]
fn a_picture_size_is_read_from_the_header_of_png_jpeg_and_bmp() {
    assert_eq!(picture_size(&png(640, 480, b"")), Some((640, 480)));
    assert_eq!(picture_size(&jpeg(1200, 900)), Some((1200, 900)));
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&[0; 16]);
    bmp.extend_from_slice(&300_i32.to_le_bytes());
    bmp.extend_from_slice(&(-200_i32).to_le_bytes());
    assert_eq!(picture_size(&bmp), Some((300, 200)));
    assert_eq!(picture_size(b"GIF89a......"), None);
    assert_eq!(picture_size(b"%PDF-1.5"), None);
    assert_eq!(picture_size(&png(1, 1, b"")[..20]), None);
}

#[test]
fn a_picture_that_would_unpack_to_a_huge_image_is_refused_before_the_engine_sees_it() {
    assert!(is_readable_picture(&png(4000, 3000, b"")));
    // 50,000 by 50,000 pixels is a few hundred bytes of file and gigabytes of memory.
    assert!(!is_readable_picture(&png(50_000, 50_000, b"")));
    assert!(!is_readable_picture(&png(9000, 9000, b"")));
    assert!(!is_readable_picture(&png(0, 100, b"")));
    assert!(!is_readable_picture(&jpeg(65_535, 65_535)));
    assert!(!is_readable_picture(b"just some text"));
}

#[test]
fn a_language_is_a_few_plain_codes_and_never_something_that_reads_as_an_option() {
    for good in ["eng", "nld+eng", "chi_sim", "eng+nld+deu+fra"] {
        assert!(valid_language(good), "{good}");
    }
    for bad in [
        "",
        "-psm",
        "--tessdata-dir",
        "eng+",
        "+eng",
        "e",
        "eng nld",
        "../x",
        "eng;calc",
        "eng+nld+deu+fra+spa",
    ] {
        assert!(!valid_language(bad), "{bad:?}");
    }
}

#[test]
fn the_engine_is_found_only_in_absolute_directories_never_in_the_current_one() {
    let directory = scratch();
    assert_eq!(locate_in(std::slice::from_ref(&directory)), None);
    let name = if cfg!(windows) {
        "tesseract.exe"
    } else {
        "tesseract"
    };
    std::fs::write(directory.join(name), b"").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        locate_in(std::slice::from_ref(&directory)),
        Some(directory.join(name))
    );
    // A relative entry (such as the current directory) is never searched, even when it holds the file.
    let relative = PathBuf::from(format!("jocr-rel-{}", jarvis_core::scratch_tag()));
    std::fs::create_dir_all(&relative).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(relative.join(name), b"").unwrap_or_else(|error| panic!("{error}"));
    let found = locate_in(&[relative.clone(), PathBuf::new()]);
    let _ = std::fs::remove_dir_all(&relative);
    assert_eq!(found, None);
}

#[tokio::test]
async fn the_picture_goes_in_on_standard_input_with_fixed_arguments() {
    let directory = scratch();
    let engine = echoing_engine(&directory);
    let picture = png(100, 100, b"\nsecret-payload-line\n");
    let text = recognise(&engine, &picture, Some("nld+eng"), Duration::from_secs(20))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(text.contains("args:stdin stdout -l nld+eng"), "{text}");
    assert!(text.contains("secret-payload-line"), "{text}");
    let plain = recognise(&engine, &picture, None, Duration::from_secs(20))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(plain.contains("args:stdin stdout"), "{plain}");
    assert!(!plain.contains("stdout -l"), "{plain}");
}

#[tokio::test]
async fn a_bad_picture_or_language_never_starts_the_engine() {
    let directory = scratch();
    let engine = echoing_engine(&directory);
    let limit = Duration::from_secs(20);
    assert_eq!(
        recognise(&engine, b"not a picture", None, limit).await,
        Err(OcrError::UnreadablePicture)
    );
    assert_eq!(
        recognise(&engine, &png(100, 100, b""), Some("--oem"), limit).await,
        Err(OcrError::BadLanguage)
    );
    // A program that does not exist fails, it does not fall through to another.
    assert_eq!(
        recognise(&directory.join("nothing"), &png(10, 10, b""), None, limit).await,
        Err(OcrError::Failed)
    );
}

#[tokio::test]
async fn an_engine_that_hangs_is_stopped_at_the_limit() {
    let directory = scratch();
    let engine = sleeping_engine(&directory);
    let started = std::time::Instant::now();
    let outcome = recognise(&engine, &png(10, 10, b""), None, Duration::from_millis(400)).await;
    assert_eq!(outcome, Err(OcrError::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(6));
}

#[test]
fn the_text_is_plain_lines() {
    assert_eq!(
        clean("  Hello \u{7}world  \r\n\r\nline two\t \n\n"),
        "Hello world\n\nline two"
    );
}
