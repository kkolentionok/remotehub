//! `Option<Vec<u8>>` <-> optional base64 string serde adapter.
//!
//! Used via `#[serde(with = "crate::opt_b64")]`. Serializes `None` as
//! JSON `null` and `Some(bytes)` as a base64 string.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
    match bytes {
        Some(b) => s.serialize_some(&STANDARD.encode(b)),
        None => s.serialize_none(),
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
    let opt = Option::<String>::deserialize(d)?;
    match opt {
        Some(text) => STANDARD
            .decode(text.as_bytes())
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Wrap {
        #[serde(with = "crate::opt_b64")]
        b: Option<Vec<u8>>,
    }

    #[test]
    fn handles_none_and_some() {
        let none = Wrap { b: None };
        let j = serde_json::to_string(&none).unwrap();
        assert!(j.contains("\"b\":null"));
        assert_eq!(serde_json::from_str::<Wrap>(&j).unwrap(), none);

        let some = Wrap { b: Some(vec![9, 9]) };
        let j = serde_json::to_string(&some).unwrap();
        let back: Wrap = serde_json::from_str(&j).unwrap();
        assert_eq!(some, back);
    }
}
