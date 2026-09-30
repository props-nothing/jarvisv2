//! Base64, in the two forms this crate's providers declare.
//!
//! # Why this is hand-written and why it is here
//!
//! The encoder was previously a private function in `auth.rs`, which is where the crate's first need for it
//! lived (RFC 7636's S256 challenge). A second need — decoding a Pub/Sub push notification body (`ADR-0088`) —
//! made it two functions in two modules, so it moved to one module rather than being copied. The **encoder is
//! unchanged**, including its alphabet, so the RFC 7636 Appendix A test that reaches it through
//! `PkceVerifier` still guards the same twelve lines.
//!
//! Hand-written rather than a dependency, for the reason the original carried: this crate needs a handful of
//! lines and a `base64` crate would arrive with an API surface — and, for a security-relevant encoding, a second
//! opinion about strictness — larger than what is used here.
//!
//! # The two alphabets, and why a decoder has to care
//!
//! RFC 4648 §4 defines **standard** base64 (`+` and `/`); §5 defines the **URL-safe** variant (`-` and `_`).
//! They differ in exactly two characters, so a value containing neither decodes identically under both — which
//! is why an alphabet confusion survives every test written against a convenient example and surfaces only on a
//! value that needs the difference. Google declares the same field in both alphabets on two pages
//! (`ADR-0088`), which is precisely that situation.

/// The standard alphabet, RFC 4648 §4.
const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The URL-safe alphabet, RFC 4648 §5.
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Why a base64 value could not be decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Base64Error {
    /// The value is too long to bound a decode.
    #[error("a base64 value must be at most {maximum} characters")]
    TooLong {
        /// The bound that was exceeded.
        maximum: usize,
    },
    /// A character was not in the alphabet the caller selected.
    #[error(
        "a base64 value may hold only `A-Z`, `a-z`, `0-9`, and the alphabet's two extra characters"
    )]
    Alphabet,
    /// Padding was present, which neither form this crate accepts carries.
    #[error("a base64 value must not be padded; this crate decodes the unpadded form only")]
    Padded,
    /// The length could not be a whole number of base64 groups.
    #[error(
        "a base64 value's length is impossible: a value of `n + 1` characters cannot encode `n` bytes"
    )]
    Length,
}

/// The longest base64 value this crate will decode, in characters.
///
/// A bound rather than a policy: a notification body is provider-supplied and reaches a decode loop, so an
/// unbounded input is an unbounded allocation. 64 KiB is far larger than any Pub/Sub notification (whose own
/// limit is 10 MB for the whole message, but whose Gmail payload is `{"emailAddress":…,"historyId":…}`) while
/// still refusing a value that is obviously not one.
pub const MAX_BASE64_CHARS: usize = 64 * 1024;

/// Encodes with the URL-safe alphabet and **no padding**, as RFC 7636 and RFC 4648 §5 require.
///
/// One encoder, used by the PKCE verifier and its challenge, because RFC 7636 §4.2 defines the challenge as
/// `BASE64URL-ENCODE(SHA256(ASCII(verifier)))` — the *same* encoding applied twice to different bytes, so a
/// second implementation would be a second chance to disagree.
#[must_use]
pub fn url_safe_no_pad(input: &[u8]) -> String {
    encode(input, URL_SAFE)
}

/// Encodes `input` with a given alphabet, without padding.
fn encode(input: &[u8], alphabet: &[u8; 64]) -> String {
    // Three input bytes become four output characters; `div_ceil` is the exact count of *unpadded* characters,
    // and it is spelled out rather than called because `div_ceil` is not const-stable on the pinned toolchain.
    let mut output =
        String::with_capacity((input.len() / 3 + usize::from(!input.len().is_multiple_of(3))) * 4);
    for chunk in input.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).copied().map_or(0, u32::from);
        let third = chunk.get(2).copied().map_or(0, u32::from);
        let block = (first << 16) | (second << 8) | third;
        // Each output character takes six bits; a chunk of `n` bytes yields `n + 1` characters, which is
        // exactly what dropping the padding means.
        for position in 0..=chunk.len() {
            let index = (block >> (18 - 6 * position)) & 0b11_1111;
            let character = alphabet[usize::try_from(index).unwrap_or(0)];
            output.push(char::from(character));
        }
    }
    output
}

/// Decodes an **unpadded URL-safe** base64 value (RFC 4648 §5).
///
/// # Errors
///
/// Returns a [`Base64Error`] for padding, an out-of-alphabet character, an impossible length, or an oversized
/// value. **Padding is refused rather than tolerated**, because the caller that uses this is a PKCE verifier
/// comparing against a value the provider normalised — and accepting padded input there would let a verifier
/// and its challenge disagree about which bytes they mean.
pub fn decode_url_safe(input: &str) -> Result<Vec<u8>, Base64Error> {
    decode(input, URL_SAFE)
}

/// Decodes an **unpadded standard** base64 value (RFC 4648 §4).
///
/// # Errors
///
/// As [`decode_url_safe`]. Padding is refused here too, for the same reason: a decoder that quietly stripped
/// `=` would accept two spellings of one value, and the caller cannot then say which it received.
pub fn decode_standard(input: &str) -> Result<Vec<u8>, Base64Error> {
    decode(input, STANDARD)
}

/// Decodes `input` with a given alphabet, refusing padding.
fn decode(input: &str, alphabet: &[u8; 64]) -> Result<Vec<u8>, Base64Error> {
    if input.len() > MAX_BASE64_CHARS {
        return Err(Base64Error::TooLong {
            maximum: MAX_BASE64_CHARS,
        });
    }
    if input.contains('=') {
        return Err(Base64Error::Padded);
    }
    // A base64 group is four characters, so a remainder of one is impossible: no whole number of bytes encodes
    // to a single leftover character. The other three remainders are legal and mean 1, 2 and 3 output bytes.
    if input.len() % 4 == 1 {
        return Err(Base64Error::Length);
    }
    let mut output = Vec::with_capacity(input.len() / 4 * 3 + 3);
    // Six bits are taken at a time into a 24-bit accumulator; every four characters (or the trailing partial
    // group) flush the whole bytes they completed.
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for character in input.bytes() {
        let value = index_of(character, alphabet).ok_or(Base64Error::Alphabet)?;
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            // The byte just completed is the top `bits + 8` bits' worth; shifting down by the remaining `bits`
            // discards the not-yet-complete trailing bits.
            let byte = (accumulator >> bits) & 0xFF;
            output.push(u8::try_from(byte).unwrap_or(0));
        }
    }
    Ok(output)
}

/// Returns a character's value in an alphabet, or `None` when it is not in it.
fn index_of(character: u8, alphabet: &[u8; 64]) -> Option<u8> {
    alphabet
        .iter()
        .position(|candidate| *candidate == character)
        .and_then(|position| u8::try_from(position).ok())
}
