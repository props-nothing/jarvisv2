//! Speech synthesis: text in, audio out, through a hosted provider.
//!
//! The provider's key is held here, in the daemon, and never reaches a browser or any other client: the page asks the
//! daemon to speak and gets audio back. See `docs/research/integrations/elevenlabs.md` (the text-to-speech section) for
//! the sources behind each decision and `docs/adr/0138-the-voice-is-synthesized-by-the-daemon.md` for the boundary.

use std::fmt;
use std::time::Duration;

use bytes::Bytes;

/// The most characters one request may speak. A long answer is spoken in several requests, which also lets the
/// first sentence start playing while the rest is still being made.
pub const MAX_TEXT_CHARS: usize = 1_500;

/// The most audio bytes accepted back for one request, so a misbehaving endpoint cannot fill memory.
pub const MAX_AUDIO_BYTES: usize = 8 * 1024 * 1024;
/// The most of the preceding sentence sent for continuity: the provider needs the tone of the last words, not the paragraph.
pub const MAX_PREVIOUS_CHARS: usize = 300;

/// The provider endpoint used unless a test or a regional deployment names another.
pub const ELEVENLABS_BASE_URL: &str = "https://api.elevenlabs.io";

/// The model used unless configuration names another: the provider's real-time expressive model.
pub const DEFAULT_MODEL: &str = "eleven_v4_turbo";

/// The voice used unless configuration names another: "George", the voice the provider's own quickstart uses.
pub const DEFAULT_VOICE_ID: &str = "JBFqnCBsd6RMkjVDRZzb";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

/// Why speech could not be produced. None of these carries the key or the provider's raw body.
#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    /// The text was empty after trimming.
    #[error("there is nothing to say")]
    EmptyText,
    /// The text was longer than [`MAX_TEXT_CHARS`].
    #[error("the text is longer than {MAX_TEXT_CHARS} characters; speak it in parts")]
    TooLong,
    /// A setting was not usable (a malformed key, voice id, model or address).
    #[error("the speech settings are not usable: {0}")]
    Settings(&'static str),
    /// The provider could not be reached or did not answer in time.
    #[error("the speech provider could not be reached")]
    Unreachable,
    /// The provider refused the key (a 401 or 403).
    #[error("the speech provider rejected the key; check the key file")]
    KeyRejected,
    /// The account is out of credit or over a rate limit (a 402 or 429).
    #[error("the speech provider says the account is out of credit or rate limited")]
    OverLimit,
    /// The provider answered with another failure status.
    #[error("the speech provider answered with status {0}")]
    Provider(u16),
    /// The audio was larger than [`MAX_AUDIO_BYTES`] or empty.
    #[error("the speech provider returned unusable audio")]
    BadAudio,
}

/// A key, kept out of `Debug` output so it cannot reach a log or an error.
#[derive(Clone)]
struct ApiKey(String);

impl fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiKey(<redacted>)")
    }
}

/// Audio produced for one request.
#[derive(Clone, Debug)]
pub struct SpeechAudio {
    bytes: Bytes,
}

impl SpeechAudio {
    /// The encoded audio (MP3).
    #[must_use]
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// The media type of [`Self::bytes`].
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        "audio/mpeg"
    }
}

/// Speech synthesis through the text-to-speech endpoint of `ElevenLabs`.
#[derive(Clone, Debug)]
pub struct ElevenLabsSpeech {
    client: reqwest::Client,
    base_url: String,
    voice_id: String,
    model_id: String,
    key: ApiKey,
}

impl ElevenLabsSpeech {
    /// Builds the adapter.
    ///
    /// # Errors
    ///
    /// Returns [`SpeechError::Settings`] for a key with whitespace or non-ASCII (it would split or corrupt a header),
    /// a voice id or model that is not a plain identifier (it becomes part of a URL path or body), or an address that
    /// is not an `http` or `https` origin.
    pub fn new(
        base_url: &str,
        voice_id: &str,
        model_id: &str,
        key: &str,
    ) -> Result<Self, SpeechError> {
        let key = key.trim();
        if key.is_empty() || key.len() > 256 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(SpeechError::Settings("the key is empty or malformed"));
        }
        let plain = |value: &str| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        };
        if !plain(voice_id) {
            return Err(SpeechError::Settings(
                "the voice id must be letters, digits, '-' or '_'",
            ));
        }
        if !plain(model_id) {
            return Err(SpeechError::Settings(
                "the model must be letters, digits, '-' or '_'",
            ));
        }
        let base = base_url.trim_end_matches('/');
        if !(base.starts_with("https://") || base.starts_with("http://"))
            || base.contains(['?', '#', ' '])
        {
            return Err(SpeechError::Settings(
                "the address must be an http(s) origin",
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| SpeechError::Settings("the HTTP client could not be built"))?;
        Ok(Self {
            client,
            base_url: base.to_owned(),
            voice_id: voice_id.to_owned(),
            model_id: model_id.to_owned(),
            key: ApiKey(key.to_owned()),
        })
    }

    /// The voice speech is made in, for display.
    #[must_use]
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }

    /// The model speech is made with, for display.
    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Turns text into audio, all of it, before returning.
    ///
    /// # Errors
    ///
    /// Returns a [`SpeechError`]: bad text, an unreachable or refusing provider, or unusable audio.
    pub async fn synthesize(&self, text: &str) -> Result<SpeechAudio, SpeechError> {
        let mut stream = self.open(text, None).await?;
        let mut audio: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next_chunk().await? {
            audio.extend_from_slice(&chunk);
        }
        if audio.is_empty() {
            return Err(SpeechError::BadAudio);
        }
        Ok(SpeechAudio {
            bytes: Bytes::from(audio),
        })
    }

    /// Starts synthesis and returns once the provider has accepted it, with the audio still arriving.
    ///
    /// The provider's `/stream` endpoint sends audio as it is generated, so the first chunk can be played long before the last
    /// exists; that is the difference between waiting for a sentence to be made and hearing it begin (about 0.2 s against about
    /// 0.6 s for a short sentence on `eleven_v4_turbo`, measured). `previous_text` is the sentence spoken just before, which the
    /// provider uses to keep the voice's intonation continuous across separate requests. It is left out for the `eleven_v3`
    /// models, which refuse it (a 400 `unsupported_model`, verified), and trimmed to the last [`MAX_PREVIOUS_CHARS`] characters.
    ///
    /// # Errors
    ///
    /// Returns a [`SpeechError`] for bad text or when the provider does not accept the request. A failure after this returns is
    /// reported by [`SpeechStream::next_chunk`].
    pub async fn open(
        &self,
        text: &str,
        previous_text: Option<&str>,
    ) -> Result<SpeechStream, SpeechError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(SpeechError::EmptyText);
        }
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(SpeechError::TooLong);
        }
        let url = format!(
            "{}/v1/text-to-speech/{}/stream",
            self.base_url, self.voice_id
        );
        let mut body = serde_json::json!({ "text": text, "model_id": self.model_id });
        if let Some(previous) = previous_text
            .map(str::trim)
            .filter(|value| !value.is_empty())
            && !self.model_id.starts_with("eleven_v3")
        {
            let skip = previous.chars().count().saturating_sub(MAX_PREVIOUS_CHARS);
            body["previous_text"] =
                serde_json::Value::String(previous.chars().skip(skip).collect());
        }
        let response = self
            .client
            .post(url)
            .header("xi-api-key", &self.key.0)
            .header(reqwest::header::ACCEPT, "audio/mpeg")
            .json(&body)
            .send()
            .await
            .map_err(|_| SpeechError::Unreachable)?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 | 403 => SpeechError::KeyRejected,
                402 | 429 => SpeechError::OverLimit,
                other => SpeechError::Provider(other),
            });
        }
        Ok(SpeechStream {
            response,
            received: 0,
        })
    }
}

/// Audio on its way from the provider.
#[derive(Debug)]
pub struct SpeechStream {
    response: reqwest::Response,
    received: usize,
}

impl SpeechStream {
    /// The next piece of audio, or `None` when the provider has finished.
    ///
    /// # Errors
    ///
    /// Returns [`SpeechError::Unreachable`] when the connection fails part-way, and [`SpeechError::BadAudio`] when the audio
    /// would exceed [`MAX_AUDIO_BYTES`], which ends the stream rather than letting it grow without bound.
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>, SpeechError> {
        match self
            .response
            .chunk()
            .await
            .map_err(|_| SpeechError::Unreachable)?
        {
            None => Ok(None),
            Some(chunk) => {
                self.received += chunk.len();
                if self.received > MAX_AUDIO_BYTES {
                    return Err(SpeechError::BadAudio);
                }
                Ok(Some(chunk))
            }
        }
    }

    /// The MIME type of the audio.
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        "audio/mpeg"
    }
}

#[cfg(test)]
mod tests;
