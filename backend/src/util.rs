//! Small helpers: time, randomness, sanitizers and JavaScript-compatible
//! value coercions used to keep the JSON API identical to the Node backend.

use std::time::{SystemTime, UNIX_EPOCH};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// Milliseconds since the Unix epoch (`Date.now()`).
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Formats like `Date.prototype.toISOString()`: `2026-01-01T00:00:00.000Z`.
pub fn iso_from_ms(ms: i64) -> String {
    let (year, month, day) = civil_from_days(ms.div_euclid(86_400_000));
    let rest = ms.rem_euclid(86_400_000);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

pub fn now_iso() -> String {
    iso_from_ms(now_ms())
}

/// Parses ISO 8601 timestamps (`YYYY-MM-DD[THH:MM[:SS[.fff]]][Z|±HH:MM]`) to
/// epoch milliseconds. Timestamps without an offset are read as UTC.
pub fn parse_iso_ms(input: &str) -> Option<i64> {
    fn number(text: &str, range: std::ops::Range<usize>) -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())?
    }

    let text = input.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let (year, month, day) = (
        number(text, 0..4)?,
        number(text, 5..7)?,
        number(text, 8..10)?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut ms = days_from_civil(year, month, day) * 86_400_000;

    let rest = &text[10..];
    if rest.is_empty() {
        return Some(ms);
    }
    if !matches!(rest.as_bytes()[0], b'T' | b't' | b' ')
        || rest.len() < 6
        || rest.as_bytes()[3] != b':'
    {
        return None;
    }
    ms += number(rest, 1..3)? * 3_600_000 + number(rest, 4..6)? * 60_000;

    let mut tail = &rest[6..];
    if let Some(seconds) = tail.strip_prefix(':') {
        ms += number(seconds, 0..2)? * 1000;
        tail = &seconds[2..];
        if let Some(fraction) = tail.strip_prefix('.') {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let millis = format!("{:0<3}", &fraction[..digits.min(3)]);
            ms += millis.parse::<i64>().ok()?;
            tail = &fraction[digits..];
        }
    }

    match tail {
        "" | "Z" | "z" => Some(ms),
        _ => {
            let sign = match tail.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let offset = tail[1..].replace(':', "");
            if offset.len() != 4 {
                return None;
            }
            let minutes = number(&offset, 0..2)? * 60 + number(&offset, 2..4)?;
            Some(ms - sign * minutes * 60_000)
        }
    }
}

/// `size` random bytes from the OS CSPRNG, hex encoded.
pub fn random_hex(size: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = vec![0u8; size];
    getrandom::fill(&mut bytes).expect("operating system random source unavailable");
    let mut out = String::with_capacity(size * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// JavaScript truthiness of a JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `String(value || "")` for scalar JSON values.
pub fn js_str(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) if truthy(&Value::Number(number.clone())) => number.to_string(),
        Some(Value::Bool(true)) => "true".to_owned(),
        _ => String::new(),
    }
}

/// Lowercase, NFKD-decompose and drop combining marks (U+0300..U+036F).
fn fold(input: &str) -> impl Iterator<Item = char> + '_ {
    input
        .to_lowercase()
        .nfkd()
        .filter(|ch| !('\u{0300}'..='\u{036f}').contains(ch))
        .collect::<Vec<_>>()
        .into_iter()
}

pub fn normalize_admin_identity(input: &str) -> String {
    fold(input).filter(|ch| !ch.is_whitespace()).collect()
}

/// Turns any display name into a URL slug; never empty.
pub fn safe_slug(input: &str) -> String {
    let mut slug = String::new();
    for ch in fold(input) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "user".to_owned()
    } else {
        slug.to_owned()
    }
}

/// Returns the lowercased slug when it is `[a-z0-9-]{1,80}`, otherwise "".
pub fn sanitize_slug(input: &str) -> String {
    let value = input.to_lowercase();
    let valid = (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid { value } else { String::new() }
}

/// Only same-site absolute paths are allowed as a post-login destination.
pub fn sanitize_return_to(input: &str) -> String {
    if input.is_empty()
        || !input.starts_with('/')
        || input.starts_with("//")
        || input.contains("://")
    {
        return "/".to_owned();
    }
    input.to_owned()
}

/// Returns the normalized URL when `input` is an http(s) URL, otherwise "".
pub fn sanitize_http_url(input: Option<&Value>) -> String {
    let Some(Value::String(raw)) = input else {
        return String::new();
    };
    match url::Url::parse(raw.trim()) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => parsed.to_string(),
        _ => String::new(),
    }
}

pub fn infer_media_type(url: &str, explicit: Option<&Value>) -> &'static str {
    match js_str(explicit).to_lowercase().as_str() {
        "image" => return "image",
        "video" => return "video",
        _ => {}
    }

    const IMAGE_EXT: [&str; 8] = [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".bmp", ".svg",
    ];
    const VIDEO_EXT: [&str; 6] = [".mp4", ".webm", ".ogg", ".ogv", ".mov", ".m4v"];

    let lowered = url.to_lowercase();
    if IMAGE_EXT.iter().any(|ext| lowered.contains(ext)) {
        "image"
    } else if VIDEO_EXT.iter().any(|ext| lowered.contains(ext)) {
        "video"
    } else {
        ""
    }
}

/// Same escaping as JavaScript's `encodeURIComponent`.
pub fn encode_uri_component(input: &str) -> String {
    const SET: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    utf8_percent_encode(input, SET).to_string()
}

/// Comparison whose duration does not depend on where the inputs differ.
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let diff = left
        .iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    std::hint::black_box(diff) == 0
}

pub fn is_lower_hex(input: &str, len: usize) -> bool {
    input.len() == len
        && input
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn iso_formatting_matches_javascript() {
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_ms(1_767_225_600_000), "2026-01-01T00:00:00.000Z");
        assert_eq!(iso_from_ms(1_780_579_567_036), "2026-06-04T13:26:07.036Z");
        assert_eq!(iso_from_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_from_ms(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn iso_parsing_round_trips() {
        for ms in [0, 1_767_225_600_000, 1_780_579_567_036, 951_782_400_000, -1] {
            assert_eq!(parse_iso_ms(&iso_from_ms(ms)), Some(ms));
        }
        assert_eq!(parse_iso_ms("2026-01-01"), Some(1_767_225_600_000));
        assert_eq!(
            parse_iso_ms("2026-01-01T02:00:00+02:00"),
            Some(1_767_225_600_000)
        );
        assert_eq!(parse_iso_ms("2026-01-01T00:00Z"), Some(1_767_225_600_000));
        assert_eq!(
            parse_iso_ms("2026-01-01T00:00:00.5Z"),
            Some(1_767_225_600_500)
        );
        assert_eq!(parse_iso_ms(""), None);
        assert_eq!(parse_iso_ms("yesterday"), None);
        assert_eq!(parse_iso_ms("2026-13-01"), None);
    }

    #[test]
    fn random_hex_shape() {
        let token = random_hex(24);
        assert!(is_lower_hex(&token, 48));
        assert_ne!(token, random_hex(24));
    }

    #[test]
    fn slugs() {
        assert_eq!(safe_slug("Yhym"), "yhym");
        assert_eq!(safe_slug("!yhym's"), "yhym-s");
        assert_eq!(safe_slug("Zażółć Gęślą"), "zazo-c-gesla");
        assert_eq!(safe_slug("ehsanulhaquemilon.edu"), "ehsanulhaquemilon-edu");
        assert_eq!(safe_slug("***"), "user");
        assert_eq!(safe_slug(""), "user");

        assert_eq!(sanitize_slug("Yhym-2"), "yhym-2");
        assert_eq!(sanitize_slug("a b"), "");
        assert_eq!(sanitize_slug("../etc"), "");
        assert_eq!(sanitize_slug(""), "");
        assert_eq!(sanitize_slug(&"a".repeat(81)), "");
        assert_eq!(sanitize_slug("yhym\n"), "");
    }

    #[test]
    fn admin_identity() {
        assert_eq!(normalize_admin_identity(" !Yhym's "), "!yhym's");
        assert_eq!(normalize_admin_identity("Y h\tÝM"), "yhym");
    }

    #[test]
    fn return_to() {
        assert_eq!(sanitize_return_to("/profile/yhym/"), "/profile/yhym/");
        assert_eq!(sanitize_return_to(""), "/");
        assert_eq!(sanitize_return_to("//evil.example"), "/");
        assert_eq!(sanitize_return_to("https://evil.example"), "/");
        assert_eq!(sanitize_return_to("/x?next=https://evil.example"), "/");
    }

    #[test]
    fn urls_and_media() {
        assert_eq!(
            sanitize_http_url(Some(&json!(" https://Example.com/a b.png "))),
            "https://example.com/a%20b.png"
        );
        assert_eq!(sanitize_http_url(Some(&json!("javascript:alert(1)"))), "");
        assert_eq!(sanitize_http_url(Some(&json!("not a url"))), "");
        assert_eq!(sanitize_http_url(Some(&json!(5))), "");
        assert_eq!(sanitize_http_url(None), "");

        assert_eq!(infer_media_type("https://x/a.PNG?x=1", None), "image");
        assert_eq!(infer_media_type("https://x/a.webm", None), "video");
        assert_eq!(infer_media_type("https://x/a", None), "");
        assert_eq!(infer_media_type("", Some(&json!("Video"))), "video");
        assert_eq!(
            infer_media_type("https://x/a.png", Some(&json!("audio"))),
            "image"
        );
    }

    #[test]
    fn javascript_coercions() {
        assert_eq!(js_str(Some(&json!("abc"))), "abc");
        assert_eq!(js_str(Some(&json!(42))), "42");
        assert_eq!(js_str(Some(&json!(0))), "");
        assert_eq!(js_str(Some(&json!(null))), "");
        assert_eq!(js_str(None), "");
        assert!(truthy(&json!({})));
        assert!(!truthy(&json!("")));
        assert_eq!(
            encode_uri_component("access_denied&x=a b/ż"),
            "access_denied%26x%3Da%20b%2F%C5%BC"
        );
        assert_eq!(encode_uri_component("-_.!~*'()"), "-_.!~*'()");
    }

    #[test]
    fn constant_time_comparison() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(is_lower_hex("0af9", 4));
        assert!(!is_lower_hex("0AF9", 4));
        assert!(!is_lower_hex("0af", 4));
    }
}
