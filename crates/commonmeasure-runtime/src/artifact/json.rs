//! Strict ingestion before values enter the canonical hash boundary.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

const SAFE: u64 = 9_007_199_254_740_991;

struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unambiguous I-JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                if v.unsigned_abs() > SAFE {
                    return Err(E::custom("integer exceeds the I-JSON safe range"));
                }
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                if v > SAFE {
                    return Err(E::custom("integer exceeds the I-JSON safe range"));
                }
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
                if !v.is_finite() || (v.fract() == 0.0 && v.abs() > SAFE as f64) {
                    return Err(E::custom("number exceeds the I-JSON safe range"));
                }
                Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(v)) = a.next_element()? {
                    values.push(v);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut values = Map::new();
                while let Some(key) = a.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON member: {key}")));
                    }
                    let Strict(v) = a.next_value()?;
                    values.insert(key, v);
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice::<Strict>(bytes)
        .map(|v| v.0)
        .map_err(|e| format!("invalid artifact JSON: {e}"))
}

pub(super) fn validate(value: &Value) -> Result<(), String> {
    // The public API also accepts Values constructed by Rust callers.
    fn walk(value: &Value, depth: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("JSON nesting exceeds 64 levels".into());
        }
        match value {
            Value::Number(n) => {
                if n.as_u64().is_some_and(|n| n > SAFE)
                    || n.as_i64().is_some_and(|n| n.unsigned_abs() > SAFE)
                    || n.as_f64().is_none_or(|n| {
                        !n.is_finite() || (n.fract() == 0.0 && n.abs() > SAFE as f64)
                    })
                {
                    return Err("number exceeds the I-JSON safe range".into());
                }
            }
            Value::Array(a) => {
                for value in a {
                    walk(value, depth + 1)?;
                }
            }
            Value::Object(o) => {
                for value in o.values() {
                    walk(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(value, 0)
}
