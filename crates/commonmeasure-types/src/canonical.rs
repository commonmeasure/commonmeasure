//! The one JSON serialisation every sealed hash and digest is taken over:
//! the JSON Canonicalization Scheme of RFC 8785.
//!
//! A hash over a re-serialised document is only reproducible when the
//! bytes are a function of the value and nothing else. serde_json's own
//! output is not: its object order is a workspace-wide feature flag
//! (`preserve_order`), which any dependency can switch on, and its number
//! formatting is not the one a reviewer's JSON library uses. RFC 8785 fixes
//! every degree of freedom, so a reviewer with any conforming serialiser and
//! SHA-256 recomputes the same seal.
//!
//! The rule, in full:
//!
//! - no whitespace;
//! - object members sorted by the UTF-16 code units of their names, which
//!   is not the same as Rust's byte order for names outside the Basic
//!   Multilingual Plane (an emoji sorts before U+FB33 in UTF-16 and after it
//!   in UTF-8);
//! - arrays in the order given; the caller sorts an array whose order carries
//!   no meaning before handing it here;
//! - strings with `"`, `\` and the control characters below U+0020 escaped
//!   (`\b`, `\t`, `\n`, `\f`, `\r`, otherwise `\u00xx` in lower-case hex) and
//!   every other character literal;
//! - numbers as ECMAScript's `Number.prototype.toString` prints them: the
//!   shortest digits that round-trip, plain notation between 1e-6 and 1e21,
//!   exponent notation with an explicit sign outside that, `-0` as `0`.
//!
//! Every JSON number is treated as an IEEE 754 double, as the RFC requires
//! of I-JSON documents. An integer beyond 2^53 loses precision here exactly
//! as it does in any JavaScript reader; no sealed document carries one.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The RFC 8785 canonical text of `value`.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// `sha256:<hex>` over `bytes` as they stand: the form every digest in the
/// product takes.
///
/// A hash over bytes that are not a JSON document — a retrieved source's
/// text, a skill's entrypoint, an evidence log, a rule set — is this one.
/// A hash over a JSON value is [`canonical_digest`], which is this over the
/// value's canonical text (`docs/contracts/canonical-json.md`).
#[must_use]
pub fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// `sha256:<hex>` over [`canonical_json`] of `value`: the form every sealed
/// digest in the product takes.
#[must_use]
pub fn canonical_digest(value: &Value) -> String {
    sha256_digest(canonical_json(value).as_bytes())
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        // A number this cannot represent must stop the seal, not become
        // zero: a digest over a substituted value is a digest of a document
        // nobody wrote (`docs/FAIL-POLICY.md` §7). Unreachable while
        // serde_json parses every number as an f64; reachable the day
        // `arbitrary_precision` is turned on, which is exactly when a silent
        // zero would be worst.
        Value::Number(number) => write_number(
            number
                .as_f64()
                .unwrap_or_else(|| panic!("{number} has no f64 form, so it cannot be sealed")),
            out,
        ),
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut sorted: Vec<(&String, &Value)> = members.iter().collect();
            sorted.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (index, (name, member)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(name, out);
                out.push(':');
                write(member, out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            control if control < ' ' => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
}

/// ECMAScript `Number.prototype.toString` for a finite double
/// (ECMA-262 §6.1.6.1.20), which is the number rule of RFC 8785 §3.2.2.2.
///
/// The digits come from `ryu`: the shortest decimal string that reads back
/// as the same double, and where two such strings are equally close to the
/// value, the one with an even last digit, which is the tie rule ECMAScript
/// states and `f64`'s own `Display` does not follow. The notation is then
/// decided from the decimal exponent alone.
fn write_number(x: f64, out: &mut String) {
    if x == 0.0 {
        // Both zeros print as `0`: the RFC's number rule has no negative
        // zero, and a sealed document must not depend on which one a
        // computation produced.
        out.push('0');
        return;
    }
    if x < 0.0 {
        out.push('-');
    }
    let mut buffer = ryu::Buffer::new();
    let (digits, n) = shortest_digits(buffer.format_finite(x.abs()));
    // x = 0.d1..dk * 10^n, with k digits and no leading or trailing zero.
    let k = digits.len() as i32;
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        let point = n as usize;
        out.push_str(&digits[..point]);
        out.push('.');
        out.push_str(&digits[point..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n <= 0 { '-' } else { '+' });
        out.push_str(&(n - 1).abs().to_string());
    }
}

/// The significant digits of a positive `ryu` rendering and the decimal
/// exponent `n` for which the value is `0.<digits> * 10^n`.
///
/// `ryu` writes either `ddd.ddd` or `d.ddde<exp>`, always with a fraction
/// part, so `1.0` and `1e23` both arrive here and leave as the digit string
/// `1` with `n` of 1 and 24.
fn shortest_digits(rendered: &str) -> (String, i32) {
    let (mantissa, exponent) = match rendered.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().unwrap_or(0)),
        None => (rendered, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits = format!("{integer}{fraction}");
    let mut n = integer.len() as i32 + exponent;
    let leading = digits.len() - digits.trim_start_matches('0').len();
    digits.drain(..leading);
    n -= leading as i32;
    let trailing = digits.len() - digits.trim_end_matches('0').len();
    digits.truncate(digits.len() - trailing);
    (digits, n)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// RFC 8785 §3.2.3, the worked example of the whole scheme.
    #[test]
    fn the_rfcs_worked_example() {
        let input = r#"{
  "numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
  "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
  "literals": [null, true, false]
}"#;
        let value: Value = serde_json::from_str(input).expect("the example parses");
        assert_eq!(
            canonical_json(&value),
            "{\"literals\":[null,true,false],\"numbers\":[333333333.3333333,1e+30,4.5,0.002,\
             1e-27],\"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
        );
    }

    /// RFC 8785 §3.2.3, the member-ordering example: UTF-16 code unit order,
    /// under which the emoji (surrogates D83D DE00) sorts before U+FB33.
    #[test]
    fn members_sort_by_utf16_code_units() {
        let input = r#"{
  "\u20ac": "Euro Sign",
  "\r": "Carriage Return",
  "\ufb33": "Hebrew Letter Dalet With Dagesh",
  "1": "One",
  "\ud83d\ude00": "Emoji: Grinning Face",
  "\u0080": "Control",
  "\u00f6": "Latin Small Letter O With Diaeresis"
}"#;
        let value: Value = serde_json::from_str(input).expect("the example parses");
        assert_eq!(
            canonical_json(&value),
            "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{80}\":\"Control\",\
             \"\u{f6}\":\"Latin Small Letter O With Diaeresis\",\"\u{20ac}\":\"Euro Sign\",\
             \"\u{1f600}\":\"Emoji: Grinning Face\",\
             \"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
        );
    }

    /// RFC 8785 Appendix B: the ECMAScript number serialisation, by the IEEE
    /// 754 bit pattern and its expected text.
    #[test]
    fn numbers_print_as_ecmascript_does() {
        let vectors: &[(u64, &str)] = &[
            (0x0000000000000000, "0"),
            (0x8000000000000000, "0"),
            (0x0000000000000001, "5e-324"),
            (0x7fefffffffffffff, "1.7976931348623157e+308"),
            (0x4340000000000000, "9007199254740992"),
            (0xc340000000000000, "-9007199254740992"),
            (0x4430000000000000, "295147905179352830000"),
            (0x44b52d02c7e14af5, "9.999999999999997e+22"),
            (0x44b52d02c7e14af6, "1e+23"),
            (0x44b52d02c7e14af7, "1.0000000000000001e+23"),
            (0x444b1ae4d6e2ef4e, "999999999999999700000"),
            (0x444b1ae4d6e2ef4f, "999999999999999900000"),
            (0x444b1ae4d6e2ef50, "1e+21"),
            (0x3eb0c6f7a0b5ed8c, "9.999999999999997e-7"),
            (0x3eb0c6f7a0b5ed8d, "0.000001"),
            (0x41b3de4355555553, "333333333.3333332"),
            (0x41b3de4355555554, "333333333.33333325"),
            (0x41b3de4355555555, "333333333.3333333"),
            (0x41b3de4355555556, "333333333.3333334"),
            (0x41b3de4355555557, "333333333.33333343"),
            (0xbecbf647612f3696, "-0.0000033333333333333333"),
            (0x43143ff3c1cb0959, "1424953923781206.2"),
        ];
        for (bits, expected) in vectors {
            let mut out = String::new();
            write_number(f64::from_bits(*bits), &mut out);
            assert_eq!(out, *expected, "bit pattern {bits:016x}");
        }
    }

    /// Integers take the same path: a whole number below 2^53 prints as its
    /// digits, whichever serde representation carried it.
    #[test]
    fn integers_print_as_their_digits() {
        assert_eq!(canonical_json(&json!(0)), "0");
        assert_eq!(canonical_json(&json!(-7)), "-7");
        assert_eq!(
            canonical_json(&json!(1_700_000_000_000u64)),
            "1700000000000"
        );
        assert_eq!(
            canonical_json(&json!(9_007_199_254_740_992u64)),
            "9007199254740992"
        );
        assert_eq!(canonical_json(&json!(1.0)), "1");
        assert_eq!(canonical_json(&json!(100.0)), "100");
    }

    /// The order a value was built in is not part of its canonical form,
    /// under either of serde_json's map implementations.
    #[test]
    fn insertion_order_does_not_reach_the_digest() {
        let one = json!({"b": {"y": 1, "x": 2}, "a": [3, 4]});
        let other = json!({"a": [3, 4], "b": {"x": 2, "y": 1}});
        assert_eq!(canonical_json(&one), r#"{"a":[3,4],"b":{"x":2,"y":1}}"#);
        assert_eq!(canonical_digest(&one), canonical_digest(&other));
        assert_ne!(
            canonical_digest(&one),
            canonical_digest(&json!({"a": [4, 3], "b": {"x": 2, "y": 1}})),
            "array order is meaning"
        );
    }

    /// Strings keep every character the RFC leaves literal: the solidus,
    /// DEL and everything above ASCII.
    #[test]
    fn strings_escape_only_what_the_rfc_escapes() {
        assert_eq!(
            canonical_json(&json!("a/b\u{7f}\u{1}\u{1f}\u{e9}")),
            "\"a/b\u{7f}\\u0001\\u001f\u{e9}\""
        );
    }
}
