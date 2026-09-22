//! Shared encoding and comparison for fixed-size secrets.
//!
//! Extracted so two security-critical implementations cannot drift. Both [`crate::ClientCredential`]
//! and [`crate::DecisionNonce`] hold 32 random bytes rendered as lowercase hexadecimal, and both
//! need a comparison that does not leak where the first difference is. Duplicating either would mean
//! a fix to one could miss the other — and the comparison is the half where a mistake is silent.

/// Renders bytes as lowercase hexadecimal.
///
/// The output length is exactly twice the input length, which callers rely on for their bounds
/// checks: a stored value whose length was not derived this way cannot compare equal to a generated
/// one.
pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Compares two byte strings without an early exit that reveals the first mismatch.
///
/// Lengths are folded into the accumulator rather than compared first, so a length mismatch costs
/// the same as a content mismatch. The loop runs over the longer input so its duration depends on the
/// inputs' lengths and not on where they differ.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let width = left.len().max(right.len());
    for index in 0..width {
        let left_byte = left.get(index).copied().unwrap_or_default();
        let right_byte = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encoding is lowercase, fixed-width, and round-trips through a byte-per-two-chars decode.
    #[test]
    fn encoding_is_lowercase_and_fixed_width() {
        assert_eq!(encode_hex(&[]), "");
        assert_eq!(encode_hex(&[0x00]), "00");
        assert_eq!(encode_hex(&[0x0f]), "0f");
        assert_eq!(encode_hex(&[0xff]), "ff");
        assert_eq!(encode_hex(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        for length in 0..8 {
            let bytes = vec![0xa5; length];
            assert_eq!(encode_hex(&bytes).len(), length * 2);
        }
    }

    /// Equal inputs compare equal, including the empty case.
    #[test]
    fn equal_inputs_are_equal() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(&[0u8; 32], &[0u8; 32]));
    }

    /// **A difference anywhere is detected, including at the last byte and in the length.**
    ///
    /// The last byte and a length difference are the two cases a comparison that short-circuits or
    /// compares lengths first would handle differently, so both are asserted explicitly.
    #[test]
    fn any_difference_is_detected() {
        assert!(
            !constant_time_eq(b"abc", b"abd"),
            "difference at the last byte"
        );
        assert!(
            !constant_time_eq(b"abc", b"bbc"),
            "difference at the first byte"
        );
        assert!(!constant_time_eq(b"abc", b"ab"), "a shorter input");
        assert!(!constant_time_eq(b"ab", b"abc"), "a longer input");
        assert!(!constant_time_eq(b"", b"a"), "empty against non-empty");
        assert!(
            !constant_time_eq(b"00000000", b"000000000"),
            "same prefix, longer"
        );
    }
}
