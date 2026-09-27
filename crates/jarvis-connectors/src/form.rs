//! `application/x-www-form-urlencoded`: the one codec, with both directions in one place.
//!
//! # Why this is its own module rather than a helper inside one caller
//!
//! Two parts of this crate need this encoding and they need **opposite directions**: the OAuth callback
//! reader decodes a redirect's query, and the token endpoint builds a `POST` body. The rule they share is the
//! one character that differs from RFC 3986 — a space is `+` here and `%20` there — and a second implementation
//! of it is exactly how the two would drift. `P3-006a`'s rule applies to a codec as much as to a struct: when
//! two values must agree, **one function** should hold both.
//!
//! # The rule, and where it comes from
//!
//! RFC 6749 §4.1.2 (the callback) and Appendix B (the encoding) both point at HTML 4.01 §17.13.4's
//! `application/x-www-form-urlencoded`:
//!
//! > Control names and values are escaped. Space characters are replaced by `+', and then reserved characters
//! > are escaped as described in [RFC1738], section 2.2: Non-alphanumeric characters are replaced by `%HH'.
//!
//! So the alphabet that survives unescaped is **alphanumerics only** — narrower than RFC 3986's unreserved set
//! (`ALPHA / DIGIT / "-" / "." / "_" / "~"`). Three consequences are deliberate and each is asserted:
//!
//! - **A space is `+`**, not `%20`. This is the character that makes the two encodings disagree, and it is the
//!   one a caller is most likely to get wrong by reaching for the RFC 3986 encoder.
//! - **`~`, `-`, `.`, `_` are escaped** (`%7E`, `%2D`, `%2E`, `%5F`) even though RFC 3986 leaves them alone.
//!   Escaping *more* than necessary is interoperable in both directions — a server that receives `%7E` decodes
//!   the same byte — whereas escaping less is what produces a value the server reads differently.
//!   [`crate::google::request::percent_encode`] is the **other** encoder and is not interchangeable with this
//!   one; a test asserts they produce different output for the same input.
//! - **Uppercase hex**, like the RFC 3986 encoder: `%7E` rather than `%7e`.
//!
//! # What this does NOT do, and why
//!
//! HTML 4.01 also says "Line breaks are represented as `CR LF` pairs (i.e., `%0D%0A`)" — an instruction to
//! **normalise** a line break, which only works if the receiver reverses it, and a receiver that does not would
//! see a value one byte longer than the one sent. OAuth's own parameters cannot carry a line break at all: the
//! ABNF in RFC 6749 Appendix A constrains `code`, `state`, and `refresh_token` to `VSCHAR` (`%x20-7E`) and
//! `error`/`error_description` to `NQSCHAR`, none of which admit CR or LF. So a line break here is a value that
//! is already invalid, and this module **encodes it faithfully** (`%0D`, `%0A`) rather than silently rewriting
//! it — the caller's validation is what should refuse it, and a codec that altered the bytes would make that
//! refusal impossible to observe.
//!
//! # Why this is a public module
//!
//! The **decoder** has a caller today (the authorization callback). The **encoder** does not: it is what the
//! token endpoint's form `POST` body needs, and that transport is unbuilt. So the module is exported rather
//! than private, because a `pub` item inside a private module is unreachable and therefore dead code — the
//! defect `P5-001` recorded for a constant and this crate hit again when `google.rs` was still private. The
//! alternative, marking the encoder `#[cfg(test)]` until a caller exists, would hide a function the next slice
//! is supposed to use.

/// The character a space becomes, per HTML 4.01 §17.13.4.
///
/// Named rather than written as a literal at the two places it appears, because it is the one character where
/// this encoding and RFC 3986 disagree and a reader should meet it by name.
pub const SPACE_ENCODED: char = '+';

/// Why a form component could not be decoded.
///
/// Not `thiserror`: the variants carry no provider text and both callers map this into their own vocabulary
/// ([`crate::authorization::AuthRefusal`] for a callback), so a `Display` here would be a second rendering of
/// the same facts. A named enum keeps the mapping exhaustive at each call site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormError {
    /// A percent escape was truncated, non-hex, or not ASCII.
    MalformedEscape,
    /// The decoded bytes were not valid UTF-8.
    NotUtf8,
}

impl FormError {
    /// Returns a bounded explanation, safe to store in a diagnostic.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::MalformedEscape => "a percent escape in the form data was not two hex digits",
            Self::NotUtf8 => "a form component was not valid UTF-8 after decoding",
        }
    }
}

/// Encodes one component of a form: a parameter name, or a value.
///
/// See the module doc for the rule and for the three deliberate consequences. A space becomes `+`; every other
/// non-alphanumeric byte becomes `%HH` with uppercase hex; bytes above ASCII are encoded per byte, which is what
/// makes the result UTF-8 (RFC 6749 §4.1.3 requires the body to be UTF-8 encoded).
#[must_use]
pub fn encode_component(component: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(component.len());
    for byte in component.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(char::from(byte));
        } else if byte == b' ' {
            encoded.push(SPACE_ENCODED);
        } else {
            // Uppercase, matching the RFC 3986 encoder's casing so a reader comparing the two does not have to
            // decide whether the difference is meaningful.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Decodes one component of a form: a parameter name, or a value.
///
/// `+` is a space (the same rule as [`encode_component`], in the other direction), `%XX` is a byte, and the
/// decoded bytes must be valid UTF-8 — Appendix B says a parsed value "need[s] to be treated as octet
/// sequences, to be decoded using the UTF-8 character encoding scheme", so an escape producing an invalid
/// sequence is a **refusal** rather than a lossy replacement. A lossy decode would turn a corrupted `state`
/// into a *different* string that merely fails to match, which hides a transport fault behind what looks like a
/// security refusal.
///
/// # Errors
///
/// Returns [`FormError::MalformedEscape`] for a truncated or non-hex escape, and [`FormError::NotUtf8`] for
/// bytes that are not valid UTF-8.
pub fn decode_component(component: &str) -> Result<String, FormError> {
    let bytes = component.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                let escape = bytes
                    .get(index + 1..index + 3)
                    .ok_or(FormError::MalformedEscape)?;
                let text = std::str::from_utf8(escape).map_err(|_| FormError::MalformedEscape)?;
                let byte = u8::from_str_radix(text, 16).map_err(|_| FormError::MalformedEscape)?;
                decoded.push(byte);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| FormError::NotUtf8)
}

/// Renders an ordered parameter list as a form body.
///
/// The `&`-separated form of the same ordered pairs the request builders produce, which is what a caller puts
/// in an HTTP body. Ordered rather than a map for the reason [`crate::google::request::HttpRequest::query`]
/// gives: a map's iteration order would make the same logical request produce different bytes on different
/// runs, so a recorded body could not be compared and a signature over the request would be impossible.
///
/// **The name is encoded too**, not just the value. A name is this crate's own constant today, so it holds
/// nothing hazardous — but encoding only the value is the habit that lets a caller-supplied name through
/// unescaped, and `=` or `&` in a name would restructure the body exactly as it would in a query string.
#[must_use]
pub fn encode_body(parameters: &[(String, String)]) -> String {
    let pairs: Vec<String> = parameters
        .iter()
        .map(|(name, value)| format!("{}={}", encode_component(name), encode_component(value)))
        .collect();
    pairs.join("&")
}

/// The media type a form body is sent as.
///
/// A constant so the header's value appears once. RFC 6749 §4.1.3 requires the token request's body to be this
/// media type, and a transport that guessed or omitted it would produce a request the server cannot parse — a
/// failure that reads as an auth problem rather than as a missing header.
pub const CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

#[cfg(test)]
#[path = "form_tests.rs"]
mod tests;
