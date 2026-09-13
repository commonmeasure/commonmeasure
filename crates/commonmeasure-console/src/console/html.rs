//! A small value accessor shared by the console's projection code: `text`
//! reads a JSON string field, or `None` when it is absent or not a string.
//! Markup is rendered by maud, which escapes every interpolated value.

use serde_json::Value;

pub fn text(value: &Value) -> Option<&str> {
    value.as_str()
}
