//! The bits of `System.Text.Json`'s writer the etw area needs, so the files keep the C# shape byte for byte:
//! strings escaped like `JavaScriptEncoder.Default` (everything outside printable ASCII, and `" & ' + < > \` and
//! the backtick, as `\uXXXX`; `\n \r \t \b \f` and `\\` short), doubles as the shortest round-trip text, and a
//! `DateTime` in the ISO form with the fraction's trailing zeros trimmed.

use crate::base::fmt::{double_default, Culture};
use crate::base::time::DateTime;

/// A JSON string literal (with the quotes).
pub fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut buf = [0u16; 2];
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\\' => out.push_str("\\\\"),
            ' '..='~' if !matches!(c, '"' | '&' | '\'' | '+' | '<' | '>' | '`') => out.push(c),
            _ => {
                for u in c.encode_utf16(&mut buf).iter() {
                    out.push_str(&format!("\\u{:04X}", u));
                }
            }
        }
    }
    out.push('"');
    out
}

/// A JSON number for a double (`System.Text.Json`: the shortest text that reads back the same value).
pub fn double(v: f64) -> String {
    double_default(v, &Culture::invariant())
}

/// A `DateTime` as System.Text.Json writes it: "2026-11-27T10:00:00.123+03:00" (zeros of the fraction trimmed).
pub fn date(d: DateTime) -> String {
    let o = d.to_o();
    // yyyy-MM-ddTHH:mm:ss.fffffff[K]
    if o.len() < 27 {
        return string(&o);
    }
    let (head, rest) = o.split_at(19);
    let frac = &rest[1..8];
    let tail = &rest[8..];
    let trimmed = frac.trim_end_matches('0');
    let s = if trimmed.is_empty() { format!("{}{}", head, tail) } else { format!("{}.{}{}", head, trimmed, tail) };
    string(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_the_default_encoder() {
        assert_eq!(string("a\\b"), "\"a\\\\b\"");
        assert_eq!(string("Кадр"), "\"\\u041A\\u0430\\u0434\\u0440\"");
        assert_eq!(string("a+b<c>&'\""), "\"a\\u002Bb\\u003Cc\\u003E\\u0026\\u0027\\u0022\"");
        assert_eq!(string("µ"), "\"\\u00B5\"");
        assert_eq!(double(12.0), "12");
        assert_eq!(double(1.5), "1.5");
        let d = DateTime::utc(crate::base::time::DateTime::from_parts(2026, 11, 27, 10, 0, 0, crate::base::time::Kind::Utc).unwrap().ticks + 1_230_000);
        assert_eq!(date(d), "\"2026-11-27T10:00:00.123Z\"");
        let d0 = crate::base::time::DateTime::from_parts(2026, 11, 27, 10, 0, 0, crate::base::time::Kind::Utc).unwrap();
        assert_eq!(date(d0), "\"2026-11-27T10:00:00Z\"");
    }
}
