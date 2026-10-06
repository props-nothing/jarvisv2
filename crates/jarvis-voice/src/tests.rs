use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

const KEY: &str = "sk_test_0123456789abcdef";

/// Serves one canned HTTP response and returns the request it received.
async fn serve_once(status: u16, body: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("address: {error}"));
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("accept: {error}"));
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            if read == 0 {
                break;
            }
            received.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&received).into_owned();
            if let Some(split) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if received.len() >= split + 4 + length {
                    break;
                }
            }
        }
        let head = format!(
            "HTTP/1.1 {status} X\r\ncontent-type: audio/mpeg\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(&body).await;
        let _ = stream.shutdown().await;
        String::from_utf8_lossy(&received).into_owned()
    });
    (format!("http://{address}"), handle)
}

fn speech(base: &str) -> ElevenLabsSpeech {
    ElevenLabsSpeech::new(base, DEFAULT_VOICE_ID, DEFAULT_MODEL, KEY)
        .unwrap_or_else(|error| panic!("settings: {error}"))
}

#[tokio::test]
async fn a_request_has_the_documented_shape_and_returns_the_audio() {
    let (base, request) = serve_once(200, b"ID3-audio-bytes".to_vec()).await;
    let audio = speech(&base)
        .synthesize("  Good evening, sir.  ")
        .await
        .unwrap_or_else(|error| panic!("synthesize: {error}"));
    assert_eq!(audio.bytes().as_ref(), b"ID3-audio-bytes");
    assert_eq!(audio.content_type(), "audio/mpeg");

    let request = request.await.unwrap_or_default();
    assert!(
        request.starts_with(&format!(
            "POST /v1/text-to-speech/{DEFAULT_VOICE_ID}/stream HTTP/1.1"
        )),
        "{request}"
    );
    let lowered = request.to_ascii_lowercase();
    assert!(lowered.contains(&format!("xi-api-key: {}", KEY.to_ascii_lowercase())));
    assert!(
        request.contains(r#""text":"Good evening, sir.""#),
        "{request}"
    );
    assert!(request.contains(&format!(r#""model_id":"{DEFAULT_MODEL}""#)));
}

/// **The key never appears in an error or in `Debug` output**, whatever the provider says.
#[tokio::test]
async fn failures_are_mapped_and_never_carry_the_key() {
    for (status, expected) in [
        (401, "rejected the key"),
        (403, "rejected the key"),
        (429, "out of credit"),
    ] {
        let (base, _request) = serve_once(status, format!("detail {KEY}").into_bytes()).await;
        let error = speech(&base)
            .synthesize("hello")
            .await
            .err()
            .unwrap_or_else(|| panic!("{status} must fail"));
        let shown = format!("{error} {error:?}");
        assert!(shown.contains(expected), "{status}: {shown}");
        assert!(!shown.contains(KEY), "the key leaked: {shown}");
    }
    let (base, _request) = serve_once(500, Vec::new()).await;
    assert!(matches!(
        speech(&base).synthesize("hello").await,
        Err(SpeechError::Provider(500))
    ));
    assert!(!format!("{:?}", speech("http://127.0.0.1:1")).contains(KEY));
}

#[tokio::test]
async fn text_and_audio_are_bounded() {
    let adapter = speech("http://127.0.0.1:1");
    assert!(matches!(
        adapter.synthesize("   ").await,
        Err(SpeechError::EmptyText)
    ));
    let long = "a".repeat(MAX_TEXT_CHARS + 1);
    assert!(matches!(
        adapter.synthesize(&long).await,
        Err(SpeechError::TooLong)
    ));

    let (base, _request) = serve_once(200, Vec::new()).await;
    assert!(matches!(
        speech(&base).synthesize("hello").await,
        Err(SpeechError::BadAudio)
    ));
    let (base, _request) = serve_once(200, vec![0_u8; MAX_AUDIO_BYTES + 1]).await;
    assert!(matches!(
        speech(&base).synthesize("hello").await,
        Err(SpeechError::BadAudio)
    ));
}

#[tokio::test]
async fn an_unreachable_provider_is_reported_as_such() {
    assert!(matches!(
        speech("http://127.0.0.1:1").synthesize("hello").await,
        Err(SpeechError::Unreachable)
    ));
}

/// Settings that would corrupt a header, a URL path or a body are refused at construction.
#[test]
fn malformed_settings_are_refused() {
    let build = |base: &str, voice: &str, model: &str, key: &str| {
        ElevenLabsSpeech::new(base, voice, model, key)
    };
    assert!(build(ELEVENLABS_BASE_URL, DEFAULT_VOICE_ID, DEFAULT_MODEL, KEY).is_ok());
    assert!(build(ELEVENLABS_BASE_URL, DEFAULT_VOICE_ID, DEFAULT_MODEL, "").is_err());
    assert!(build(ELEVENLABS_BASE_URL, DEFAULT_VOICE_ID, DEFAULT_MODEL, "a b").is_err());
    assert!(
        build(
            ELEVENLABS_BASE_URL,
            DEFAULT_VOICE_ID,
            DEFAULT_MODEL,
            "k\r\nx: y"
        )
        .is_err()
    );
    assert!(build(ELEVENLABS_BASE_URL, "../v1/models", DEFAULT_MODEL, KEY).is_err());
    assert!(build(ELEVENLABS_BASE_URL, "", DEFAULT_MODEL, KEY).is_err());
    assert!(build(ELEVENLABS_BASE_URL, DEFAULT_VOICE_ID, "a/b", KEY).is_err());
    assert!(build("ftp://example.com", DEFAULT_VOICE_ID, DEFAULT_MODEL, KEY).is_err());
    assert!(
        build(
            "https://example.com?x=1",
            DEFAULT_VOICE_ID,
            DEFAULT_MODEL,
            KEY
        )
        .is_err()
    );
}
