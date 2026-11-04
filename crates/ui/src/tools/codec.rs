//! Text codecs. Decoding rejects malformed input and non-UTF-8 output without data loss.
use super::ToolKind;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};

pub(super) fn convert(
    kind: ToolKind,
    side: usize,
    source: &str,
    form: bool,
) -> Result<String, String> {
    match (kind, side) {
        (ToolKind::Url, 0) => Ok(url_encode(source, form)),
        (ToolKind::Url, _) => url_decode(source, form),
        (ToolKind::Base64, 0) => Ok(STANDARD.encode(source.as_bytes())),
        (ToolKind::Base64, _) => {
            let input: Vec<u8> = source
                .bytes()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            let bytes = STANDARD
                .decode(&input)
                .or_else(|_| STANDARD_NO_PAD.decode(&input))
                .map_err(|e| format!("Invalid Base64: {e}"))?;
            String::from_utf8(bytes).map_err(|_| "Decoded Base64 is not valid UTF-8 text".into())
        }
        (ToolKind::Json, _) => Err("JSON uses its own formatter".into()),
    }
}
fn url_encode(source: &str, form: bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(source.len());
    for byte in source.bytes() {
        if byte.is_ascii_alphanumeric() || (if form { b"-._*" } else { b"-._~" }).contains(&byte) {
            output.push(char::from(byte));
        } else if form && byte == b' ' {
            output.push('+');
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    output
}
const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
fn url_decode(source: &str, form: bool) -> Result<String, String> {
    let input = source.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut i = 0;
    while let Some(&byte) = input.get(i) {
        if byte == b'%' {
            let pair = input
                .get(i + 1..i + 3)
                .and_then(|pair| Some((hex(pair[0])?, hex(pair[1])?)));
            let Some((hi, lo)) = pair else {
                return Err(format!(
                    "Invalid URL escape at byte {}: expected %HH",
                    i + 1
                ));
            };
            output.push(hi * 16 + lo);
            i += 3;
        } else {
            output.push(if form && byte == b'+' { b' ' } else { byte });
            i += 1;
        }
    }
    String::from_utf8(output).map_err(|_| "Decoded URL is not valid UTF-8 text".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_unicode_reserved_characters_form_mode_and_invalid_escapes() -> Result<(), String> {
        let plain = "Привет + &=/?#%\n😀";
        for form in [false, true] {
            let encoded = convert(ToolKind::Url, 0, plain, form)?;
            assert!(encoded.starts_with("%D0%9F"));
            assert_eq!(convert(ToolKind::Url, 1, &encoded, form)?, plain);
        }
        assert_eq!(url_encode("~*", true), "%7E*");
        assert_eq!(url_encode("~*", false), "~%2A");
        assert_eq!(url_encode("a b+c", false), "a%20b%2Bc");
        assert_eq!(url_encode("a b+c", true), "a+b%2Bc");
        assert_eq!(url_decode("a+b", false)?, "a+b");
        assert_eq!(url_decode("a+b", true)?, "a b");
        assert_eq!(url_decode("%2f%2F", false)?, "//");
        for input in ["%", "%2", "%GG", "%FF", "%C0%AF"] {
            assert!(url_decode(input, false).is_err());
        }
        assert_eq!(url_encode("-._~AZaz09", false), "-._~AZaz09");
        Ok(())
    }
    #[test]
    fn base64_vectors_unicode_multiline_unpadded_and_invalid_utf8() -> Result<(), String> {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(convert(ToolKind::Base64, 0, plain, false)?, encoded);
            assert_eq!(convert(ToolKind::Base64, 1, encoded, false)?, plain);
        }
        let text = "Привет\n😀\0";
        assert_eq!(
            convert(ToolKind::Base64, 1, &STANDARD.encode(text), false)?,
            text
        );
        assert_eq!(convert(ToolKind::Base64, 1, " Z m 8 =\r\n", false)?, "fo");
        assert_eq!(convert(ToolKind::Base64, 1, "Zg", false)?, "f");
        for input in ["Z", "Zg=", "!!!", "/w==", "Zh=="] {
            assert!(convert(ToolKind::Base64, 1, input, false).is_err());
        }
        Ok(())
    }
}
