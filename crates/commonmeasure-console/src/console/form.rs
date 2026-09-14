//! Query and form decoding.
//!
//! One decoder. A query value and a form value carry the same
//! `application/x-www-form-urlencoded` encoding, and the rules editor accepts
//! any non-empty engagement name, so the two must agree: a query decoder that
//! handled less than the editor writes would fail to match names the console
//! had just saved, and report the sessions under them as nothing recorded.

use anyhow::{Result, bail};

/// One query value by key from a request target. An undecodable value reads
/// as absent, which leaves the view unfiltered: the failure a reader can see
/// is better than a filter that matches nothing and calls it silence.
pub fn query_param(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if value.is_empty() || percent_decode(name).ok()? != key {
            return None;
        }
        percent_decode(value).ok()
    })
}

/// An `application/x-www-form-urlencoded` body as ordered pairs. Order is
/// meaning here: the editor's row order becomes the rule file's precedence.
pub fn parse_form(body: &[u8]) -> Result<Vec<(String, String)>> {
    let text = std::str::from_utf8(body).map_err(|_| anyhow::anyhow!("form body is not UTF-8"))?;
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((percent_decode(name)?, percent_decode(value)?))
        })
        .collect()
}

/// Percent-encode one query value, as a browser encodes a form control's.
/// The console builds its own refresh URLs, so a name it accepted from the
/// editor has to survive being put back into a query string — HTML-escaping
/// one, which is a different job, let an `&` in a name split the query.
pub fn encode_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The inverse of [`encode_component`], and the one decoder: query values,
/// form fields and the id segments of the console's own links all carry the
/// same encoding, so reading them differently is how a link the console emitted
/// stops naming the thing it was built from.
pub fn percent_decode(encoded: &str) -> Result<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let Some(hex) = encoded.get(i + 1..i + 3) else {
                    bail!("truncated percent escape in {encoded:?}");
                };
                let Ok(byte) = u8::from_str_radix(hex, 16) else {
                    bail!("invalid percent escape %{hex} in {encoded:?}");
                };
                out.push(byte);
                i += 2;
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|_| anyhow::anyhow!("{encoded:?} decodes to invalid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_decoding_handles_paths_and_spaces() {
        let pairs =
            parse_form(b"match=code%2Fozone&engagement=oz+one&match=common%20measure").unwrap();
        assert_eq!(
            pairs,
            vec![
                ("match".into(), "code/ozone".into()),
                ("engagement".into(), "oz one".into()),
                ("match".into(), "common measure".into()),
            ]
        );
    }

    #[test]
    fn malformed_escapes_are_errors_not_guesses() {
        assert!(parse_form(b"a=%2").is_err());
        assert!(parse_form(b"a=%zz").is_err());
        assert!(parse_form(b"a=%ff").is_err()); // invalid UTF-8 once decoded
    }

    #[test]
    fn query_param_reads_what_a_browser_sends() {
        assert_eq!(
            query_param("/sessions?engagement=oz+one", "engagement").as_deref(),
            Some("oz one")
        );
        assert_eq!(query_param("/sessions", "engagement"), None);
        assert_eq!(query_param("/sessions?engagement=", "engagement"), None);
    }

    /// Percent-encode as a browser does for a form control value: everything
    /// outside the unreserved set is escaped.
    fn form_encode(value: &str) -> String {
        value
            .bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (byte as char).to_string()
                }
                _ => format!("%{byte:02X}"),
            })
            .collect()
    }

    /// The editor accepts any non-empty name, so every name it accepts must
    /// survive the trip back through the engagement filter. Two decoders that
    /// disagree here make the console report recorded sessions as "Nothing
    /// recorded yet".
    #[test]
    fn every_name_the_editor_accepts_round_trips_through_the_filter() {
        for name in [
            "ozone",
            "a&b=c",
            "50% margin",
            "oz one",
            "a+b",
            "a#b",
            "a/b",
            "réseau",
            "quote\"and'apos",
        ] {
            let target = format!("/sessions?engagement={}", form_encode(name));
            assert_eq!(
                query_param(&target, "engagement").as_deref(),
                Some(name),
                "{name} did not survive the query round trip"
            );
            // The same bytes through the editor's decoder must agree.
            let body = format!("match=x&engagement={}", form_encode(name));
            let pairs = parse_form(body.as_bytes()).unwrap();
            assert_eq!(pairs[1].1, name, "{name} did not survive the form decoder");
        }
    }
}
