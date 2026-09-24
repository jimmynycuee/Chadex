//! Streaming JSON hashing helpers for callers that consume only a SHA-256 digest.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

struct Sha256Writer<'a>(&'a mut Sha256);

impl Write for Sha256Writer<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn update_sha256_with_json<T: Serialize + ?Sized>(
    hasher: &mut Sha256,
    value: &T,
) -> Result<(), serde_json::Error> {
    let mut writer = Sha256Writer(hasher);
    serde_json::to_writer(&mut writer, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn streaming_json_sha256_matches_buffered_serialization() {
        for value in [
            json!(["plain", "quote=\" slash=\\", "Unicode 你好 🦀"]),
            json!({
                "nested": {"array": [1, true, null, "line\nnext"]},
                "unicode": "日本語 🌍",
                "escaped": "\t\r\n"
            }),
        ] {
            let buffered = Sha256::digest(serde_json::to_vec(&value).unwrap());
            let mut streaming = Sha256::new();
            update_sha256_with_json(&mut streaming, &value).unwrap();
            assert_eq!(streaming.finalize().as_slice(), buffered.as_slice());
        }
    }
}
