//! `Vec<u8>` <-> base64 (standard, padded) serde adapter.
//!
//! Used via `#[serde(with = "crate::b64")]`. Binary fields (KDF salt,
//! AEAD nonce, ciphertext) serialize as base64 strings instead of JSON
//! number arrays — smaller and human-readable in the export file.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&STANDARD.encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let text = String::deserialize(d)?;
    STANDARD
        .decode(text.as_bytes())
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Wrap {
        #[serde(with = "crate::b64")]
        a: Vec<u8>,
    }

    #[test]
    fn roundtrips_and_is_string_in_json() {
        let w = Wrap { a: vec![0, 1, 2, 250, 255] };
        let j = serde_json::to_string(&w).unwrap();
        assert!(j.contains("\"a\":\""), "bytes should encode as a JSON string");
        let back: Wrap = serde_json::from_str(&j).unwrap();
        assert_eq!(w, back);
    }
}
