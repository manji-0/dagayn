//! The `str()` of the Python exceptions the VCS helpers let escape, for the
//! error envelopes that report them.

use std::io;

/// `str(exc)` of the `OSError` `subprocess` raises when `program` cannot be
/// started: `[Errno 13] Permission denied: 'git'`.
pub fn os_error(err: &io::Error, program: &str) -> String {
    let text = err.to_string();
    match err.raw_os_error() {
        Some(code) => {
            let strerror = text
                .strip_suffix(&format!(" (os error {code})"))
                .unwrap_or(&text);
            format!("[Errno {code}] {strerror}: '{program}'")
        }
        None => text,
    }
}

/// `str(exc)` of the `UnicodeDecodeError` `bytes.decode("utf-8")` raises on
/// `bytes`, or `None` when they decode.
pub fn utf8_error(bytes: &[u8]) -> Option<String> {
    let err = std::str::from_utf8(bytes).err()?;
    let start = err.valid_up_to();
    let (end, reason) = match err.error_len() {
        None => (bytes.len(), "unexpected end of data"),
        // A lone byte that cannot open a sequence, or a sequence whose
        // continuation breaks off.
        Some(1) if !matches!(bytes[start], 0xC2..=0xF4) => (start + 1, "invalid start byte"),
        Some(len) => (start + len, "invalid continuation byte"),
    };
    Some(if end == start + 1 {
        format!(
            "'utf-8' codec can't decode byte 0x{:02x} in position {start}: {reason}",
            bytes[start]
        )
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {start}-{}: {reason}",
            end - 1
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_errors_read_as_python_words_them() {
        assert_eq!(utf8_error(b"ok"), None);
        assert_eq!(
            utf8_error(b"ab\xffcd").unwrap(),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
        assert_eq!(
            utf8_error(b"ab\xe2\x82").unwrap(),
            "'utf-8' codec can't decode bytes in position 2-3: unexpected end of data"
        );
        assert_eq!(
            utf8_error(b"\xe2\x82x").unwrap(),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        assert_eq!(
            utf8_error(b"\xe0\x80").unwrap(),
            "'utf-8' codec can't decode byte 0xe0 in position 0: invalid continuation byte"
        );
    }

    #[test]
    fn os_errors_read_as_python_words_them() {
        let err = io::Error::from_raw_os_error(13);
        assert_eq!(os_error(&err, "git"), "[Errno 13] Permission denied: 'git'");
    }
}
