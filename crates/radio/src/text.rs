//! Decoding the text radio servers send, which is in no declared encoding.

/// UTF-8 if the bytes are valid UTF-8, which is what modern broadcasting software sends, and
/// Windows-1252 otherwise, which is what the rest sends and which is Latin-1 plus printable
/// characters in the range Latin-1 leaves for controls. Trailing NUL padding is dropped.
pub(crate) fn decode(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |last| last + 1);
    let bytes = &bytes[..end];
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&byte| windows_1252(byte)).collect(),
    }
}

fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}',
        '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}',
        '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}',
        '\u{9D}', '\u{017E}', '\u{0178}',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        other => char::from(other),
    }
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn valid_utf8_is_taken_as_utf8() {
        assert_eq!(
            decode("caf\u{e9} \u{1F3B5}".as_bytes()),
            "caf\u{e9} \u{1F3B5}"
        );
    }

    #[test]
    fn anything_else_is_windows_1252() {
        assert_eq!(decode(b"caf\xe9"), "caf\u{e9}");
        // 0x96 and 0x92 are an en dash and a right quote there, but C1 controls in Latin-1.
        assert_eq!(decode(b"a\x96b\x92s"), "a\u{2013}b\u{2019}s");
        // Bytes Windows-1252 leaves unassigned stay as the control characters Latin-1 has.
        assert_eq!(decode(b"\x81"), "\u{81}");
    }

    #[test]
    fn trailing_padding_is_dropped() {
        assert_eq!(decode(b"abc\0\0\0"), "abc");
        assert_eq!(decode(b"\0\0"), "");
    }
}
