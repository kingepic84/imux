//! Standard-alphabet base64 decoding for inline image payloads.

fn value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode, skipping white space (terminals wrap long payloads) and accepting missing padding.
/// `None` for any other byte outside the alphabet.
pub fn decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in input {
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            b'=' => break,
            _ => {}
        }
        acc = (acc << 6) | u32::from(value(b)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode with padding, no line breaks.
pub fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, &b| (a << 8) | u32::from(b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        out.extend(std::iter::repeat_n('=', 3 - chunk.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_matches_the_rfc_vectors_and_roundtrips() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(encode(raw.as_bytes()), enc);
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(encode(&all).as_bytes()).unwrap(), all);
    }

    #[test]
    fn rfc4648_vectors() {
        assert_eq!(decode(b"").unwrap(), b"");
        assert_eq!(decode(b"Zg==").unwrap(), b"f");
        assert_eq!(decode(b"Zm8=").unwrap(), b"fo");
        assert_eq!(decode(b"Zm9v").unwrap(), b"foo");
        assert_eq!(decode(b"Zm9vYg==").unwrap(), b"foob");
        assert_eq!(decode(b"Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn padding_is_optional_and_white_space_ignored() {
        assert_eq!(decode(b"Zm9vYg").unwrap(), b"foob");
        assert_eq!(decode(b"Zm9v\r\nYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn bytes_outside_the_alphabet_are_refused() {
        assert_eq!(decode(b"Zm9v!"), None);
        assert_eq!(decode(b"Zm9v-_"), None);
    }
}
