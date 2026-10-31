//! Typed answers read with serde: nulls take the field's default and unknown fields are kept in
//! `extra`, so a newer API never breaks an older SDK.
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::error::{Error, Result};

fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(strip_nulls).collect()),
        other => other,
    }
}

pub(crate) fn decode<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(strip_nulls(v))
        .map_err(|e| Error::Decode(format!("the API's answer couldn't be read: {e}")))
}

pub(crate) fn list<T: DeserializeOwned>(v: &Value, key: &str) -> Result<Vec<T>> {
    crate::json::arr(v, key)
        .iter()
        .map(|x| decode(x.clone()))
        .collect()
}

/// A value shown only once, such as a new key or a webhook's signing secret. It never appears in
/// `Debug` output.
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The secret itself.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_empty() {
            "Secret(none)"
        } else {
            "Secret(..)"
        })
    }
}

/// A price or an amount, which the API may send as a string or a number.
pub(crate) fn amount<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct T {
        a: String,
        n: u32,
        s: Secret,
    }

    #[test]
    fn nulls_take_defaults_and_secrets_stay_hidden() {
        let t: T = decode(serde_json::json!({"a": null, "n": 3, "s": "shh", "new": 1})).unwrap();
        assert_eq!(t.a, "");
        assert_eq!(t.n, 3);
        assert_eq!(t.s.expose(), "shh");
        assert_eq!(format!("{:?}", t.s), "Secret(..)");
    }
}
