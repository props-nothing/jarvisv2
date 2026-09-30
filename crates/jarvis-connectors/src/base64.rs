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
    /// The padding was present but malformed.
    ///
    /// Padding is **observable** in the string — it is the `=` at the end — so it is validated rather than
    /// searched for, and a value whose padding is wrong is a corrupt value rather than one in an unexpected
    /// form. A count above two, or a padded length that is not a multiple of four, lands here.
    #[error("a base64 value's padding is malformed: at most two `=` and only after a whole group")]
    Padding,
    /// The length could not be a whole number of base64 groups.
    #[error(
        "a base64 value's length is impossible: a value of `n + 1` characters cannot encode `n` bytes"
    )]
    Length,
}

/// Which of RFC 4648's two alphabets a value is written in.
///
/// The two differ in exactly two characters (`+`/`/` versus `-`/`_`), which is why a decoder has to be **told**
/// which one it is reading rather than guessing: a value containing neither decodes identically either way, so
/// a guess cannot be checked against the result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Alphabet {
    /// RFC 4648 §5, which uses `-` and `_`.
    UrlSafe,
    /// RFC 4648 §4, which uses `+` and `/`.
    Standard,
}

impl Alphabet {
    /// Returns the alphabet's stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UrlSafe => "base64url",
            Self::Standard => "base64",
        }
    }

    /// Returns the alphabet's 64 characters.
    const fn characters(self) -> &'static [u8; 64] {
        match self {
            Self::UrlSafe => URL_SAFE,
            Self::Standard => STANDARD,
        }
    }
}

/// Whether a value carried `=` padding.
///
/// **A property of the value, not a parameter to the decoder.** Padding is *observable* — it is the `=` at the
/// end of the string — unlike the alphabet, which needs no marker and so cannot be read off a value that
/// happens to use neither of the two characters that distinguish them. So this is reported rather than
/// requested: a decoder told to expect padding would have to decide what an unpadded value means, while a
/// decoder that *looks* simply knows.
///
/// It is still worth reporting, because it is the axis that separates the two published forms of the same
/// field: the Gmail guide's example is unpadded and Cloud Pub/Sub's own is padded (`ADR-0089`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Padding {
    /// The value carried no `=`. Either it needed none, or its producer omitted them.
    Absent,
    /// The value ended with one or two `=`, which is Google's convention for a `bytes` field.
    Present,
}

impl Padding {
    /// Returns the padding's stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "unpadded",
            Self::Present => "padded",
        }
    }

    /// Returns which padding a value carries, from the value alone.
    ///
    /// The only way to be wrong here is to misread the string, which is why this is a lookup and not a guess.
    #[must_use]
    pub fn of(input: &str) -> Self {
        if input.ends_with('=') {
            Self::Present
        } else {
            Self::Absent
        }
    }
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
/// Decodes `input` in a stated alphabet, reading the padding from the value.
///
/// # Why the alphabet is a parameter and the padding is not
///
/// The two are different kinds of fact. **Padding is observable**: it is the `=` at the end, so a decoder can
/// look. **The alphabet is not**: the two differ in exactly two characters (`+`/`/` versus `-`/`_`), so a value
/// containing neither decodes identically either way and no inspection can recover which was intended. A
/// parameter is therefore needed for the unobservable one, and asking for the observable one would let a caller
/// claim a padding the value does not have — a refusal invented rather than found (`ADR-0089`).
///
/// # Errors
///
/// Returns [`Base64Error::Padding`] for malformed padding, [`Base64Error::Alphabet`] for a character outside the
/// stated alphabet, [`Base64Error::Length`] for an impossible length, and [`Base64Error::TooLong`] past the
/// bound.
pub fn decode_with(input: &str, alphabet: Alphabet) -> Result<Vec<u8>, Base64Error> {
    if input.len() > MAX_BASE64_CHARS {
        return Err(Base64Error::TooLong {
            maximum: MAX_BASE64_CHARS,
        });
    }
    // Padding is validated, not searched for. A `=` is not in either alphabet, so it is stripped **before** the
    // character loop — and a value whose padding is malformed is refused here rather than reported as an
    // alphabet fault, which would name the wrong problem (`ADR-0086`'s defect in a different place).
    //
    // The padding itself is **discarded** rather than returned: it does not change the arithmetic (the decoder
    // stops emitting bytes when fewer than eight bits remain, so trailing `=` could only repeat a byte already
    // emitted), and a caller that wants to know it asks [`Padding::of`]. Returning it here would put the same
    // fact in two places, one of which a mutation could change without any test noticing.
    let (body, _) = split_padding(input)?;
    // A base64 group is four characters, so a remainder of one is impossible: no whole number of bytes encodes
    // to a single leftover character. The other three remainders are legal and mean 1, 2 and 3 output bytes
    // (or 2 and 1 once padding is accounted for).
    if body.len() % 4 == 1 {
        return Err(Base64Error::Length);
    }
    let characters = alphabet.characters();
    let mut output = Vec::with_capacity(body.len() / 4 * 3 + 3);
    // Six bits are taken at a time into a 24-bit accumulator; every four characters (or the trailing partial
    // group) flush the whole bytes they completed.
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for character in body.bytes() {
        let value = index_of(character, characters).ok_or(Base64Error::Alphabet)?;
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

/// Splits a value into its unpadded body and which padding it carried.
///
/// Validates that padding is at most two `=` **and only at the end**, which is what RFC 4648 §4 permits: a
/// padded value's length is always a multiple of four, so a value with padding anywhere else is corrupt rather
/// than in an unexpected form.
fn split_padding(input: &str) -> Result<(&str, Padding), Base64Error> {
    let body = input.trim_end_matches('=');
    let padding = input.len() - body.len();
    if padding == 0 {
        return Ok((input, Padding::Absent));
    }
    if padding > 2 || !input.len().is_multiple_of(4) {
        return Err(Base64Error::Padding);
    }
    Ok((body, Padding::Present))
}

/// Returns a character's value in an alphabet, or `None` when it is not in it.
fn index_of(character: u8, alphabet: &[u8; 64]) -> Option<u8> {
    alphabet
        .iter()
        .position(|candidate| *candidate == character)
        .and_then(|position| u8::try_from(position).ok())
}
