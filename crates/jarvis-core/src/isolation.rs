//! Isolation of untrusted text before it reaches a model prompt.
//!
//! # Why this is a separate concern from trust classification
//!
//! `jarvis_core::context` decides **whether** untrusted content may be included, and requires it to be
//! marked quoted. `jarvis_core::memory` decides **what trust a stored claim carries**, from its origin.
//! Neither decides **how** untrusted text is presented once it is included — and that is where an
//! injection actually lands, because the model sees one flat string and the marking is only a decision
//! recorded beside it.
//!
//! `docs/architecture/security.md` treats retrieved content and tool output as data rather than
//! authority, and `docs/architecture/memory-and-context.md` requires external content be "marked as
//! untrusted data" with "any instructions inside it isolated". Marking is what the two modules do;
//! isolation is what this one does.
//!
//! # What isolation can and cannot be
//!
//! **It cannot be a filter.** Deciding whether a sentence is an instruction requires understanding it,
//! and a classifier that could do that reliably would be the same model whose behaviour is at risk. Any
//! pattern list is defeated by rephrasing, by a language the list does not cover, or by text the author
//! never imagined. So this module does not try to detect instructions and does not pretend to.
//!
//! **It is instead three properties the text is made to have**, each of which an attacker cannot undo:
//!
//! 1. **Neutralised characters.** Bidirectional overrides, zero-width joiners, and other format
//!    characters change how text *reads* without changing what it *is*. In a prompt where the model
//!    reads a transcript, text that displays one thing and encodes another is a deception primitive
//!    rather than a typographic nicety — the same reasoning `jarvis-mcp`'s tool-description sanitiser
//!    applies, and the same ranges.
//! 2. **A structure that cannot be closed from inside.** The payload is fenced with a boundary token,
//!    and any occurrence of that token *in the payload* is removed, so the text cannot end its own
//!    quarantine and continue as though it were outside it.
//! 3. **A stated framing.** The instruction that the fenced region is data is itself **authoritative**
//!    text this platform wrote, not part of the payload. A model that follows it treats the contents as
//!    something to read; a model that ignores it is no worse off than it would have been with the raw
//!    text, because nothing in the fencing removed meaning.
//!
//! Property 3 is why the fence is not itself a security claim. The claim is properties 1 and 2: the text
//! cannot *look* like something else, and it cannot *escape* the region it is placed in. Both hold
//! regardless of what the model decides to do with the content.
//!
//! # Honest limits
//!
//! - **A model may still follow an instruction inside the fence.** Prompt injection is mitigated here,
//!   not solved, and no fencing scheme solves it. What protects the system is that the *effect* of any
//!   instruction still passes through policy, authorization, and approval — the model may ask, and
//!   deterministic Rust decides.
//! - **Detecting an attempt is not attempted.** [`looks_like_an_instruction`] exists to make a
//!   *reportable* observation for an operator, and its doc says plainly that a `false` is not evidence of
//!   safety. It is deliberately not wired into any refusal path.
//! - **The neutralisation is lossy.** A memory containing a zero-width joiner loses it. That is accepted
//!   because the alternative — carrying a character that can reorder the text around it into a prompt — is
//!   worse, and because the stored memory is unchanged: this transforms a *rendering* for one request.

use std::fmt;

use thiserror::Error;

/// The boundary token that fences one untrusted payload.
///
/// Chosen to be improbable in ordinary text and to be a single token with no spaces, so it survives
/// tokenization as one unit rather than as three unrelated pieces.
pub const FENCE_TOKEN: &str = "JARVIS-UNTRUSTED-DATA";

/// The opening marker written before a payload.
pub const FENCE_OPEN: &str = "<<JARVIS-UNTRUSTED-DATA>>";
/// The closing marker written after a payload.
pub const FENCE_CLOSE: &str = "<<END-JARVIS-UNTRUSTED-DATA>>";

/// Maximum characters of one isolated payload.
///
/// A bound is required rather than optional: the fence is written into a request whose size is bounded
/// by a budget, and an unbounded payload would be a way to spend that budget with a single item. Set to
/// the memory content bound so a payload cannot exceed what a memory can hold.
pub const MAX_ISOLATED_CHARS: usize = 4096;

/// Why a payload could not be isolated.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum IsolationError {
    /// The payload was empty or whitespace only.
    #[error("an isolated payload cannot be empty")]
    Empty,
    /// The payload exceeded [`MAX_ISOLATED_CHARS`] after neutralisation.
    #[error("an isolated payload exceeds {MAX_ISOLATED_CHARS} characters")]
    TooLong {
        /// The payload's length in characters, after neutralisation.
        characters: usize,
    },
}

/// Untrusted text that has been neutralised and fenced, ready to be placed in a prompt.
///
/// # Why this is a type rather than two functions
///
/// The three properties have to hold *together*: a payload whose characters were neutralised but which
/// was never fenced has property 1 and not 2, and a caller that renders it directly would have no way to
/// know which it holds. Constructing this type is the only way to obtain text that carries both, so
/// "isolated" is a property of the value rather than a claim about the call order.
///
/// The **fence markers are not part of the payload's own text**. [`Self::body`] returns the neutralised
/// payload alone, and [`Self::render`] writes the fenced form. Separating them means a caller that wants
/// to show a user what was retrieved — an inspect or export surface — is not forced to display the
/// machine framing, while a caller building a prompt cannot accidentally get the unfenced form because
/// the fenced one is what the type renders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolatedText {
    body: String,
    /// Characters removed during neutralisation, so a caller can record that the payload was altered.
    removed: usize,
    /// Whether the payload contained its own copy of the fence token.
    contained_fence: bool,
}

impl IsolatedText {
    /// Neutralises and fences an untrusted payload.
    ///
    /// # Errors
    ///
    /// Returns [`IsolationError::Empty`] for blank input and [`IsolationError::TooLong`] when the
    /// neutralised payload exceeds [`MAX_ISOLATED_CHARS`].
    ///
    /// The length check is against the **neutralised** length, which is why it runs after the transform:
    /// checking the original would let a payload expand past the bound, and it is the value that goes
    /// into the request that has to be bounded.
    pub fn new(payload: &str) -> Result<Self, IsolationError> {
        let mut body = String::with_capacity(payload.len().min(MAX_ISOLATED_CHARS * 4));
        let mut removed = 0_usize;
        for character in payload.chars() {
            // **A newline and a tab are structure, not formatting.** The first draft removed every
            // control character, which silently turned a multi-line memory into one run-on line — and a
            // list rendered as a sentence is a different claim. The two are kept; a carriage return is not,
            // because a lone one is a line-overwrite primitive and a CRLF is equivalent to the LF beside it.
            if character == '\n' || character == '\t' {
                body.push(character);
                continue;
            }
            if character.is_control() || is_format_character(character) {
                // Dropped rather than replaced with a space: a control character is not a word
                // boundary, and inserting one would change the text's word structure in a way an
                // attacker could use to split or merge terms.
                removed += 1;
                continue;
            }
            body.push(character);
        }
        // Trailing and leading whitespace is trimmed so the fence does not sit on its own line with
        // padding that reads as content.
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return Err(IsolationError::Empty);
        }

        // The fence token cannot appear in the payload. This is the property that makes the region
        // unclosable from inside, and it is a **removal** rather than an escape: escaping would need the
        // model to understand the escape, while removal needs nothing of it.
        let contained_fence = contains_fence_token(trimmed);
        let body = if contained_fence {
            remove_fence_token(trimmed)
        } else {
            trimmed.to_owned()
        };

        let characters = body.chars().count();
        if characters > MAX_ISOLATED_CHARS {
            return Err(IsolationError::TooLong { characters });
        }
        if body.trim().is_empty() {
            // A payload that was *only* a fence token has nothing left, which is the same as empty.
            return Err(IsolationError::Empty);
        }
        Ok(Self {
            body,
            removed,
            contained_fence,
        })
    }

    /// Returns the neutralised payload, without the fence.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Returns how many characters neutralisation removed.
    #[must_use]
    pub const fn removed_characters(&self) -> usize {
        self.removed
    }

    /// Returns whether the payload contained the fence token and had it removed.
    ///
    /// A caller records this rather than treating it as a refusal. Text that mentions the fence token is
    /// not necessarily hostile — a memory recording a previous conversation about this mechanism would
    /// contain it — and refusing would make a legitimate subject unrecordable. What the flag buys is that
    /// the alteration is **visible** instead of silent.
    #[must_use]
    pub const fn contained_fence(&self) -> bool {
        self.contained_fence
    }

    /// Returns whether neutralisation changed the payload at all.
    #[must_use]
    pub const fn was_altered(&self) -> bool {
        self.removed > 0 || self.contained_fence
    }

    /// Renders the fenced form, which is what goes into a prompt.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered =
            String::with_capacity(self.body.len() + FENCE_OPEN.len() + FENCE_CLOSE.len() + 2);
        rendered.push_str(FENCE_OPEN);
        rendered.push('\n');
        rendered.push_str(&self.body);
        rendered.push('\n');
        rendered.push_str(FENCE_CLOSE);
        rendered
    }
}

impl fmt::Display for IsolatedText {
    /// Renders the fenced form, so a `format!` of this value cannot produce the unfenced one.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

/// Returns whether a character is a format character that can change how text reads.
///
/// The ranges are the bidirectional embedding, override, and isolate characters, the directional marks,
/// the zero-width space and the non-joiner/joiner, and the word joiner. All of them are invisible in a
/// rendered prompt yet reorder or join text around them, so a payload containing one can *display* as one
/// thing while the model receives another.
///
/// This is the same range set `jarvis-mcp` applies to a tool's own description, repeated here rather than
/// shared because `jarvis-mcp` depends on `jarvis-core` and not the reverse — a dependency in that
/// direction would put a protocol crate under the domain. The duplication is a real cost, and it is
/// recorded as one: the two lists must be changed together, and a test in each crate pins the ranges.
#[must_use]
pub const fn is_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}'
    )
}

/// Returns whether text contains the fence token, ignoring case and separators.
///
/// Case-insensitive and separator-tolerant because the token is made of ordinary letters and hyphens: a
/// payload that writes it in lower case, or with a space for the hyphen, would read to a model as the
/// same marker while a byte comparison would miss it.
///
/// **The needle is normalised the same way as the haystack.** The first version stripped separators from
/// the text and then searched for a token that still had its own hyphens in it, so the comparison could
/// never match — and the check silently did nothing for every payload, including the real marker. A
/// test that asserted the *marker count* in the rendering is what found it; the assertions written in
/// terms of the same normalisation had reproduced the bug and passed.
fn contains_fence_token(text: &str) -> bool {
    normalise_for_comparison(text).contains(&normalised_needle())
}

/// Returns the fence token with separators removed and case folded, for a comparison.
fn normalised_needle() -> String {
    normalise_for_comparison(FENCE_TOKEN)
}

/// Removes every occurrence of the fence token, ignoring case and separators.
fn remove_fence_token(text: &str) -> String {
    // The needle for the scan is the separator-free form, because the scan skips separators in the
    // payload — comparing against a needle that still had them would fail at the first hyphen.
    let needle: Vec<u8> = normalised_needle().into_bytes();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0_usize;
    // The scan runs against the *raw* text so the removal preserves the characters it does not match. A
    // byte scan is safe because every byte it compares is ASCII, so a match can only begin at an ASCII
    // byte and cannot split a multi-byte character.
    while index < bytes.len() {
        if let Some(length) = fence_token_at(bytes, index, &needle) {
            // Replaced with a space rather than removed outright, so the words on either side do not
            // become one word.
            out.push(' ');
            index += length;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or('\u{fffd}');
        out.push(character);
        index += character.len_utf8();
    }
    out
}

/// Returns the byte length of a fence token starting at `index`, if one does.
///
/// Separators are allowed between the token's own characters but not before it, so the match is anchored
/// and a longer run of ordinary words cannot be swallowed.
fn fence_token_at(bytes: &[u8], index: usize, needle: &[u8]) -> Option<usize> {
    let mut needle_index = 0_usize;
    let mut cursor = index;
    while needle_index < needle.len() {
        // Skip separators between the token's characters, but never before the first one.
        if needle_index > 0 {
            while cursor < bytes.len() && is_separator(bytes[cursor]) {
                cursor += 1;
            }
        }
        if cursor >= bytes.len() {
            return None;
        }
        if !bytes[cursor].eq_ignore_ascii_case(&needle[needle_index]) {
            return None;
        }
        cursor += 1;
        needle_index += 1;
    }
    // A trailing separator is not part of the match, so a token followed by a space removes the token and
    // leaves the space.
    Some(cursor - index)
}

/// Returns whether a byte is a separator that may appear inside a spelled-out fence token.
const fn is_separator(byte: u8) -> bool {
    matches!(byte, b'-' | b'_' | b' ' | b'\t')
}

/// Returns a copy of the text with case folded and separators dropped, for a containment check.
fn normalise_for_comparison(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_ascii() && !is_separator(*character as u8))
        .map(|character| character.to_ascii_uppercase())
        .collect()
}

/// Phrases that *may* indicate an attempt to instruct, for an operator report.
///
/// # This is not a detector, and a `false` is not evidence of safety
///
/// The list exists so an operator reviewing a retrieval can see that content *mentioned* something
/// instruction-shaped. It is not a filter and is not wired into any refusal, for the reason the module
/// doc gives: detecting an instruction requires understanding it, any list is defeated by rephrasing or
/// by a language the list does not cover, and a list that refused on a match would make an ordinary
/// conversation *about* prompt injection unrecordable — including this platform's own notes.
///
/// The honest use is: `true` means "worth a look", `false` means "this list found nothing", and neither
/// is a statement about whether the content is safe.
#[must_use]
pub fn looks_like_an_instruction(text: &str) -> bool {
    /// Substrings checked in case-folded form. Kept short and specific; a broad list produces noise that
    /// trains an operator to ignore the flag.
    const MARKERS: [&str; 8] = [
        "ignore previous",
        "ignore all previous",
        "disregard the above",
        "disregard previous",
        "you are now",
        "system:",
        "new instructions",
        "override your",
    ];
    let folded = text.to_lowercase();
    MARKERS.iter().any(|marker| folded.contains(marker))
}

#[cfg(test)]
#[path = "isolation/tests.rs"]
mod tests;
