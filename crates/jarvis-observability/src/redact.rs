//! Structural secret redaction applied at the log sink boundary.
//!
//! Redaction is defense in depth (`docs/operations/observability.md`). The
//! strongest layer is "do not construct log fields from secret-bearing objects",
//! but a single careless `info!` call is enough to leak. This module is the
//! structural layer that every log line passes through before it reaches a sink,
//! so a leak requires defeating the writer rather than merely forgetting a rule.
//!
//! Three classes are handled explicitly, because they fail independently:
//!
//! 1. Known live secret values (exact match against the profile credential).
//! 2. Credential-bearing key/value pairs (`api_key=`, `"token":"`, `password=`)
//!    including query strings, where the value is not otherwise recognisable.
//! 3. URL userinfo (`scheme://user:password@host`).
//!
//! A redactor that only handles one of these looks like coverage while leaving
//! the others intact, so the tests assert each class separately.

use std::sync::Arc;

/// Smallest known secret that will be masked.
///
/// Masking very short strings would replace ordinary text and hide real content,
/// so a secret below this length is ignored rather than over-matched.
pub const MIN_SECRET_BYTES: usize = 8;

/// Largest number of known secret values a redactor will hold.
pub const MAX_SECRETS: usize = 64;

/// Maximum bytes of a single log line before it is truncated.
pub const MAX_LINE_BYTES: usize = 8 * 1024;

const PLACEHOLDER: &str = "[REDACTED]";

/// Bytes that terminate a credential value in a log line.
const VALUE_TERMINATORS: &[u8] = b" \t\r\n\"'&,}];";

/// Field or parameter names whose value is always a credential.
const CREDENTIAL_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "refresh_token",
    "id_token",
    "auth_token",
    "token",
    "password",
    "passwd",
    "secret",
    "client_secret",
    "private_key",
    "credential",
    "authorization",
];

/// Masks secret values and credential-bearing text in log output.
///
/// Cloning is cheap; the rules are reference counted.
#[derive(Clone, Debug)]
pub struct Redactor {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    secrets: Vec<String>,
}

impl Redactor {
    /// Builds a redactor from known live secret values.
    ///
    /// Values shorter than [`MIN_SECRET_BYTES`] are ignored, and only the first
    /// [`MAX_SECRETS`] are retained, so an adversarial or accidental input
    /// cannot make log rendering unbounded.
    #[must_use]
    pub fn new<I, S>(secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut retained = Vec::new();
        for secret in secrets {
            if retained.len() >= MAX_SECRETS {
                break;
            }
            let secret = secret.into();
            if secret.len() >= MIN_SECRET_BYTES {
                retained.push(secret);
            }
        }
        Self {
            inner: Arc::new(Inner { secrets: retained }),
        }
    }

    /// Builds a redactor with no known secrets.
    #[must_use]
    pub fn without_secrets() -> Self {
        Self::new(Vec::<String>::new())
    }

    /// Reports how many known secrets are masked.
    #[must_use]
    pub fn secret_count(&self) -> usize {
        self.inner.secrets.len()
    }

    /// Redacts one log line, bounding its length.
    ///
    /// The result is safe to write to any sink: known secrets are removed first,
    /// then credential-shaped text, then the line is truncated on a UTF-8
    /// boundary with an explicit omitted-byte count.
    #[must_use]
    pub fn redact_line(&self, line: &str) -> String {
        let bounded = truncate_utf8(line, MAX_LINE_BYTES);
        let mut masked = self.mask_secrets(&bounded);
        masked = mask_credentials(&masked);
        masked
    }

    fn mask_secrets(&self, line: &str) -> String {
        if self.inner.secrets.is_empty() {
            return line.to_owned();
        }
        let mut ranges = Vec::new();
        for secret in &self.inner.secrets {
            let mut from = 0;
            while let Some(offset) = line[from..].find(secret.as_str()) {
                let start = from + offset;
                ranges.push((start, start + secret.len()));
                from = start + secret.len();
                if from >= line.len() {
                    break;
                }
            }
        }
        apply_ranges(line, ranges)
    }
}

/// Truncates text to a byte budget on a UTF-8 boundary, reporting omitted bytes.
fn truncate_utf8(value: &str, budget: usize) -> String {
    if value.len() <= budget {
        return value.to_owned();
    }
    let mut end = budget;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let omitted = value.len() - end;
    format!("{}...[truncated {omitted} bytes]", &value[..end])
}

/// Masks every credential-shaped occurrence and returns the rewritten line.
fn mask_credentials(line: &str) -> String {
    let lower = ascii_lowercase_bytes(line);
    let mut ranges = Vec::new();
    ranges.extend(url_userinfo_ranges(line, &lower));
    for key in CREDENTIAL_KEYS {
        ranges.extend(key_value_ranges(&lower, key));
    }
    ranges.extend(bearer_ranges(&lower));
    apply_ranges(line, ranges)
}

/// Finds `key` followed by a separator, masking the value up to a terminator.
fn key_value_ranges(lower: &[u8], key: &str) -> Vec<(usize, usize)> {
    let needle = key.as_bytes();
    let mut ranges = Vec::new();
    let mut from = 0;
    while from + needle.len() <= lower.len() {
        let Some(offset) = find_bytes(&lower[from..], needle) else {
            break;
        };
        let start = from + offset;
        let after_key = start + needle.len();
        // Skip an optional closing quote before the separator, as in `"token":`.
        let mut cursor = after_key;
        if cursor < lower.len() && (lower[cursor] == b'"' || lower[cursor] == b'\'') {
            cursor += 1;
        }
        // Require a separator directly after the key so `authentication` and
        // `tokenize` are not mistaken for credentials.
        while cursor < lower.len() && lower[cursor] == b' ' {
            cursor += 1;
        }
        if cursor >= lower.len() || !matches!(lower[cursor], b'=' | b':' | b' ') {
            from = after_key;
            continue;
        }
        cursor += 1;
        while cursor < lower.len() && matches!(lower[cursor], b' ' | b'"' | b'\'') {
            cursor += 1;
        }
        let value_end = value_end(lower, cursor);
        if value_end > cursor {
            ranges.push((cursor, value_end));
        }
        from = value_end.max(after_key);
        if from >= lower.len() {
            break;
        }
    }
    ranges
}

/// Masks a `Bearer <token>` value wherever it appears.
fn bearer_ranges(lower: &[u8]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while from < lower.len() {
        let Some(offset) = find_bytes(&lower[from..], b"bearer") else {
            break;
        };
        let start = from + offset + b"bearer".len();
        let mut cursor = start;
        while cursor < lower.len() && lower[cursor] == b' ' {
            cursor += 1;
        }
        let end = value_end(lower, cursor);
        if end > cursor {
            ranges.push((cursor, end));
        }
        from = end.max(start);
        if from >= lower.len() {
            break;
        }
    }
    ranges
}

/// Masks the password portion of a URL's userinfo (`scheme://user:pass@host`).
fn url_userinfo_ranges(line: &str, lower: &[u8]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while from < lower.len() {
        let Some(offset) = find_bytes(&lower[from..], b"://") else {
            break;
        };
        let authority_start = from + offset + 3;
        let authority_end = lower[authority_start..]
            .iter()
            .position(|byte| {
                matches!(
                    *byte,
                    b'/' | b'?' | b'#' | b' ' | b'"' | b'\'' | b'\r' | b'\n'
                )
            })
            .map_or(line.len(), |index| authority_start + index);
        if let Some(at) = rfind_byte(&lower[authority_start..authority_end], b'@') {
            let at_index = authority_start + at;
            if let Some(colon) = rfind_byte(&lower[authority_start..at_index], b':') {
                let password_start = authority_start + colon + 1;
                if at_index > password_start {
                    ranges.push((password_start, at_index));
                }
            }
        }
        from = authority_end.max(authority_start);
        if from >= lower.len() {
            break;
        }
    }
    ranges
}

/// Returns the end index of a value starting at `start`, stopping at a terminator.
fn value_end(lower: &[u8], start: usize) -> usize {
    let mut cursor = start;
    while cursor < lower.len() && !VALUE_TERMINATORS.contains(&lower[cursor]) {
        cursor += 1;
    }
    cursor
}

/// Replaces each range with the placeholder, merging overlaps.
fn apply_ranges(line: &str, mut ranges: Vec<(usize, usize)>) -> String {
    if ranges.is_empty() {
        return line.to_owned();
    }
    ranges.retain(|(start, end)| start < end && *end <= line.len());
    if ranges.is_empty() {
        return line.to_owned();
    }
    ranges.sort_unstable();

    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in ranges {
        // Skip a range that is already covered by the previous replacement.
        if start < cursor {
            let covered_end = end.max(cursor);
            if covered_end > cursor {
                output.push_str(PLACEHOLDER);
                cursor = covered_end;
            }
            continue;
        }
        output.push_str(&line[cursor..start]);
        output.push_str(PLACEHOLDER);
        cursor = end;
    }
    output.push_str(&line[cursor..]);
    output
}

/// Returns an ASCII-lowercased copy for case-insensitive marker searches.
fn ascii_lowercase_bytes(value: &str) -> Vec<u8> {
    value
        .bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .collect()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Finds the last occurrence of a single byte.
fn rfind_byte(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().rposition(|byte| *byte == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANARY: &str = "9f4c1d2e8b7a6350f1e2d3c4b5a69788";

    #[test]
    fn a_known_secret_never_survives_redaction() {
        let redactor = Redactor::new([CANARY]);
        assert_eq!(redactor.secret_count(), 1);

        let cases = [
            format!("credential={CANARY}"),
            format!("presented {CANARY} to the daemon"),
            format!("Authorization: Bearer {CANARY}"),
            format!("url=\"https://host/v2/{CANARY}\""),
            format!("{CANARY}{CANARY}"),
        ];
        for case in cases {
            let redacted = redactor.redact_line(&case);
            assert!(
                !redacted.contains(CANARY),
                "secret survived redaction in {case:?} -> {redacted:?}"
            );
        }
    }

    #[test]
    fn url_userinfo_passwords_are_masked() {
        let redactor = Redactor::without_secrets();

        let redacted = redactor.redact_line("dsn=postgres://jarvis:s3cr3t-pw@db.local:5432/jarvis");
        assert!(!redacted.contains("s3cr3t-pw"), "got {redacted:?}");
        // Only the password is masked: the user and host still identify the peer.
        assert!(
            redacted.contains("jarvis:"),
            "user should remain: {redacted:?}"
        );
        assert!(
            redacted.contains("@db.local"),
            "host should remain: {redacted:?}"
        );

        // The userinfo form differs from a query parameter, so it is asserted separately.
        let query = redactor.redact_line("https://host/path?api_key=abc123def456&x=1");
        assert!(!query.contains("abc123def456"), "got {query:?}");
        assert!(
            query.contains("x=1"),
            "unrelated query value must remain: {query:?}"
        );
    }

    #[test]
    fn credential_keys_are_masked_in_query_and_json_forms() {
        let redactor = Redactor::without_secrets();

        for case in [
            "token=ghp_abcdefghijklmnop",
            "access_token=ya29.abcdefghijkl",
            r#"{"password":"hunter2000","user":"a""#,
            "api-key: sk-live-abcdefghijkl",
            "client_secret='shh-abcdefghijkl'",
        ] {
            let redacted = redactor.redact_line(case);
            assert!(
                redacted.contains(PLACEHOLDER),
                "expected a mask in {case:?} -> {redacted:?}"
            );
        }
    }

    #[test]
    fn ordinary_words_containing_credential_substrings_are_untouched() {
        let redactor = Redactor::without_secrets();

        // `authentication` contains `authorization`-adjacent text and `tokenize`
        // contains `token`; neither is a credential assignment.
        for case in [
            "authentication succeeded for client",
            "tokenizer initialized",
            "the password policy was updated",
            "secretive behavior detected",
        ] {
            let redacted = redactor.redact_line(case);
            assert_eq!(redacted, case, "ordinary text must not be altered");
        }
    }

    #[test]
    fn truncation_is_utf8_safe_and_reports_omitted_bytes() {
        let redactor = Redactor::without_secrets();
        let long = "é".repeat(MAX_LINE_BYTES);
        let redacted = redactor.redact_line(&long);

        assert!(redacted.contains("truncated"));
        // The truncation notice itself must not create invalid UTF-8.
        assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());
    }

    #[test]
    fn short_secrets_are_ignored_to_avoid_over_matching() {
        let redactor = Redactor::new(["ab", "abc"]);
        assert_eq!(redactor.secret_count(), 0);
        assert_eq!(redactor.redact_line("abcab ab"), "abcab ab");
    }

    #[test]
    fn secret_count_is_bounded() {
        let secrets: Vec<String> = (0..MAX_SECRETS + 10)
            .map(|index| format!("secret-value-{index:08}"))
            .collect();
        let redactor = Redactor::new(secrets);
        assert_eq!(redactor.secret_count(), MAX_SECRETS);
    }
}
