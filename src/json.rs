//! Reading answers forgivingly: missing fields take their defaults, unknown ones are ignored.
use serde_json::{Map, Value};

pub(crate) fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

pub(crate) fn st(v: &Value, k: &str) -> String {
    s(v, k).unwrap_or_default()
}

pub(crate) fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

pub(crate) fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}

pub(crate) fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

pub(crate) fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

pub(crate) fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub(crate) fn strings(v: &Value, k: &str) -> Vec<String> {
    arr(v, k)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

pub(crate) fn obj(v: &Value, k: &str) -> Value {
    match v.get(k) {
        Some(o @ Value::Object(_)) => o.clone(),
        _ => Value::Object(Map::new()),
    }
}

pub(crate) fn bbox(v: Option<&Value>) -> [i64; 4] {
    let mut out = [0; 4];
    if let Some(a) = v.and_then(Value::as_array) {
        for (o, n) in out.iter_mut().zip(a) {
            *o = n.as_f64().map_or(0, |x| x.round() as i64);
        }
    }
    out
}
