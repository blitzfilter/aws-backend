use time::{OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Replaces hyphens used as time-part separators with colons, leaving the
/// date hyphens untouched.
///
/// Some sites emit malformed ISO 8601 like `2026-03-03T07-30-00Z` where the
/// time separator is `-` instead of `:`. We detect this by checking whether
/// a `T` or space is present followed by `HH-MM` and fix only the time part.
///
/// Examples
/// - `2026-03-03T07-30-00Z`      → `2026-03-03T07:30:00Z`
/// - `2026-03-03T07-30-00+02-00` → `2026-03-03T07:30:00+02:00`
/// - `2026-03-03T07-30-00+02:00` → `2026-03-03T07:30:00+02:00`
fn fix_hyphen_time_separators(s: &str) -> Option<String> {
    // Find the T (or space used as T) separating date from time.
    let t_pos = s.find(['T', 't'])?;
    let time_part = &s[t_pos + 1..];

    // Must look like HH-MM to be a candidate (digits, then hyphen at position 2).
    // Avoid modifying already-valid strings or offset signs like +02-00.
    let bytes = time_part.as_bytes();
    if bytes.len() < 5
        || !bytes[0].is_ascii_digit()
        || !bytes[1].is_ascii_digit()
        || bytes[2] != b'-'
    {
        return None;
    }

    // Replace hyphens in the time portion only. We do this character by
    // character so that we can distinguish the time hyphens from offset sign
    // hyphens.  The time portion ends at Z, +, or - that starts an offset
    // (which appears after at least HH:MM has been consumed).
    let date_part = &s[..=t_pos];
    let mut result = String::with_capacity(s.len());
    result.push_str(date_part);

    // State machine: replace the first two hyphens (HH-MM-SS) then keep the rest.
    let mut replaced = 0u8;
    for c in time_part.chars() {
        if c == '-' && replaced < 2 {
            result.push(':');
            replaced += 1;
        } else {
            result.push(c);
        }
    }

    Some(result)
}

/// Strips fractional seconds from an RFC 3339 string so that the `time` crate
/// can parse it via `Rfc3339`. The crate's `Rfc3339` parser requires the
/// sub-second part to be parsed separately; stripping it is the simplest
/// portable approach.
///
/// `2024-06-01T10:00:00.123Z` → `2024-06-01T10:00:00Z`
fn strip_fractional_seconds(s: &str) -> Option<String> {
    // Look for a dot after a time-like pattern: …T or …t followed by HH:MM:SS.
    let dot_pos = s.find('.')?;
    // Must be preceded by at least HH:MM:SS (8 chars) after the T.
    // Quick sanity: character before dot must be a digit (part of seconds).
    if !s[..dot_pos].ends_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // Find end of fractional part: first non-digit after the dot.
    let after_dot = &s[dot_pos + 1..];
    let frac_len = after_dot
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_dot.len());
    if frac_len == 0 {
        return None;
    }
    // Rebuild without the fractional part.
    let mut out = String::with_capacity(s.len());
    out.push_str(&s[..dot_pos]);
    out.push_str(&s[dot_pos + 1 + frac_len..]);
    Some(out)
}

/// Normalises a bare numeric UTC offset that has no colon, e.g. `+0200` or
/// `-0500`, into the `+02:00` / `-05:00` form that `Rfc3339` expects.
///
/// Only rewrites the offset; the rest of the string is unchanged.
fn fix_compact_offset(s: &str) -> Option<String> {
    // Find a `+` or `-` that introduces a compact offset: sign + 4 digits at
    // the very end of the string, and the character before the sign must be a
    // digit (end of seconds field).
    let sign_pos = s.rfind(['+', '-'])?;
    // Must not be the leading sign of a negative Unix epoch or a date hyphen.
    if sign_pos == 0 {
        return None;
    }
    if !s[..sign_pos].ends_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let offset_part = &s[sign_pos + 1..];
    // Must be exactly 4 digits with no colon.
    if offset_part.len() != 4
        || offset_part.contains(':')
        || !offset_part.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let (hh, mm) = offset_part.split_at(2);
    let mut out = String::with_capacity(s.len() + 1);
    out.push_str(&s[..sign_pos + 1]); // include the sign
    out.push_str(hh);
    out.push(':');
    out.push_str(mm);
    Some(out)
}

/// Attempts to parse `s` (already trimmed) as RFC 3339 after optionally
/// replacing a space separator with `T`.
fn try_rfc3339(s: &str) -> Option<OffsetDateTime> {
    // Direct attempt.
    if let Ok(dt) = OffsetDateTime::parse(s, &Rfc3339) {
        return Some(dt);
    }
    // Space instead of T: "2024-06-01 10:00:00+02:00"
    if s.contains(' ')
        && let Ok(dt) = OffsetDateTime::parse(&s.replacen(' ', "T", 1), &Rfc3339)
    {
        return Some(dt);
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Attempts to parse a datetime string using a series of well-known formats.
///
/// Supported formats (in order of attempt):
///
/// **With explicit timezone / offset (parsed as-is):**
///  1. RFC 3339 / ISO 8601 with offset or Z   (`2024-06-01T10:00:00+02:00`, `…Z`)
///  2. RFC 3339 with fractional seconds        (`2024-06-01T10:00:00.123Z`)
///  3. RFC 3339 with compact offset            (`2024-06-01T10:00:00+0200`)
///  4. Space separator instead of T            (`2024-06-01 10:00:00+02:00`)
///  5. Space separator + UTC literal           (`2024-06-01 10:00:00 UTC`)
///  6. Hyphenated time parts                   (`2026-03-03T07-30-00Z`, `…+02-00`)
///  7. ISO 8601 basic (compact)                (`20240601T100000Z`, `20240601T100000+0200`)
///  8. Day-MonthName-Year + time + TZ abbrev   (`18 Apr. 2026 15:00 CEST`)
///
/// **Without timezone info (assumed UTC – no timezone context is available):**
///  9. ISO 8601 date-only                      (`2024-06-01`)              → midnight UTC
/// 10. SQL datetime with seconds               (`2024-06-01 10:30:00`)     → UTC
/// 11. SQL datetime without seconds            (`2024-06-01 10:30`)        → UTC
/// 12. German datetime with seconds            (`01.06.2024 10:30:00`)     → UTC
/// 13. German datetime without seconds         (`01.06.2024 10:30`)        → UTC
/// 14. German date-only                        (`01.06.2024`)              → midnight UTC
/// 15. US datetime with seconds                (`06/01/2024 10:30:00`)     → UTC
/// 16. US datetime without seconds             (`06/01/2024 10:30`)        → UTC
/// 17. US date-only                            (`06/01/2024`)              → midnight UTC
/// 18. Unix epoch                              (`1717228800`)
pub fn parse_datetime(raw: &str) -> Option<OffsetDateTime> {
    let s = raw.trim();

    // ------------------------------------------------------------------
    // 1. RFC 3339 (includes "space instead of T" variant)
    // ------------------------------------------------------------------
    if let Some(dt) = try_rfc3339(s) {
        return Some(dt);
    }

    // ------------------------------------------------------------------
    // 2. RFC 3339 with fractional seconds – strip fractions then retry
    // ------------------------------------------------------------------
    if let Some(stripped) = strip_fractional_seconds(s)
        && let Some(dt) = try_rfc3339(&stripped)
    {
        return Some(dt);
    }

    // ------------------------------------------------------------------
    // 3. RFC 3339 with compact offset (+0200 / -0500, no colon)
    // ------------------------------------------------------------------
    if let Some(fixed) = fix_compact_offset(s) {
        if let Some(dt) = try_rfc3339(&fixed) {
            return Some(dt);
        }
        // Also try fractional + compact together.
        if let Some(stripped) = strip_fractional_seconds(&fixed)
            && let Some(dt) = try_rfc3339(&stripped)
        {
            return Some(dt);
        }
    }

    // ------------------------------------------------------------------
    // 5. Space separator + literal " UTC" suffix
    //    e.g. "2024-06-01 10:00:00 UTC"
    // ------------------------------------------------------------------
    if let Some(base) = s.strip_suffix(" UTC").or_else(|| s.strip_suffix(" utc"))
        && let Some(dt) = try_rfc3339(&format!("{}Z", base.replacen(' ', "T", 1)))
    {
        return Some(dt);
    }

    // ------------------------------------------------------------------
    // 6. Hyphenated time parts: "2026-03-03T07-30-00Z" / "+02-00"
    //    Apply fix, then re-run all offset-aware attempts.
    // ------------------------------------------------------------------
    if let Some(fixed) = fix_hyphen_time_separators(s) {
        if let Some(dt) = try_rfc3339(&fixed) {
            return Some(dt);
        }
        if let Some(stripped) = strip_fractional_seconds(&fixed)
            && let Some(dt) = try_rfc3339(&stripped)
        {
            return Some(dt);
        }
        if let Some(fixed2) = fix_compact_offset(&fixed)
            && let Some(dt) = try_rfc3339(&fixed2)
        {
            return Some(dt);
        }
    }

    // ------------------------------------------------------------------
    // 7. ISO 8601 basic (compact) format: "20240601T100000Z" / "+0200"
    //    Rewrite to extended form: "2024-06-01T10:00:00Z"
    // ------------------------------------------------------------------
    if let Some(dt) = parse_iso_basic(s) {
        return Some(dt);
    }

    // ------------------------------------------------------------------
    // 8. "DD Mon[.] YYYY HH:MM[:SS] [TZ]"
    //    e.g. "18 Apr. 2026 15:00 CEST", "18 Apr 2026 15:00 CET"
    // ------------------------------------------------------------------
    if let Some(dt) = parse_day_mon_year_time_tz(s) {
        return Some(dt);
    }

    // ------------------------------------------------------------------
    // Timezone-naive formats below.
    // The caller provides no timezone context, so UTC is assumed.
    // ------------------------------------------------------------------

    // 9. ISO 8601 date-only "YYYY-MM-DD"
    {
        let fmt = format_description!("[year]-[month]-[day]");
        if let Ok(date) = time::Date::parse(s, &fmt) {
            return Some(date.midnight().assume_utc());
        }
    }

    // 10. "YYYY-MM-DD HH:MM:SS"
    {
        let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 11. "YYYY-MM-DD HH:MM"
    {
        let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 12. "DD.MM.YYYY HH:MM:SS"
    {
        let fmt = format_description!("[day].[month].[year] [hour]:[minute]:[second]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 13. "DD.MM.YYYY HH:MM"
    {
        let fmt = format_description!("[day].[month].[year] [hour]:[minute]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 14. "DD.MM.YYYY"
    {
        let fmt = format_description!("[day].[month].[year]");
        if let Ok(date) = time::Date::parse(s, &fmt) {
            return Some(date.midnight().assume_utc());
        }
    }

    // 15. "MM/DD/YYYY HH:MM:SS"
    {
        let fmt = format_description!("[month]/[day]/[year] [hour]:[minute]:[second]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 16. "MM/DD/YYYY HH:MM"
    {
        let fmt = format_description!("[month]/[day]/[year] [hour]:[minute]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 17. "MM/DD/YYYY"
    {
        let fmt = format_description!("[month]/[day]/[year]");
        if let Ok(date) = time::Date::parse(s, &fmt) {
            return Some(date.midnight().assume_utc());
        }
    }

    // 18. Unix epoch (integer seconds)
    if let Ok(epoch) = s.parse::<i64>() {
        return OffsetDateTime::from_unix_timestamp(epoch).ok();
    }

    None
}

/// Returns the 1-based month number for a 3-letter (or full) English month
/// name.  A trailing period is stripped before matching, so `"Apr."` and
/// `"Apr"` are treated identically.  Matching is case-insensitive.
fn parse_month_abbrev(s: &str) -> Option<u8> {
    match s.trim_end_matches('.').to_ascii_lowercase().as_str() {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

/// Maps well-known timezone abbreviations to their UTC offset in whole seconds.
///
/// The list covers the most common abbreviations encountered on European and
/// North-American auction sites.  It intentionally omits ambiguous
/// abbreviations (e.g. `IST` which can mean Indian, Irish, or Israeli time).
fn tz_abbrev_to_offset_secs(tz: &str) -> Option<i32> {
    match tz.to_ascii_uppercase().as_str() {
        // UTC / GMT
        "UTC" | "GMT" | "Z" => Some(0),
        // Western Europe
        "WET" => Some(0),
        "WEST" | "BST" => Some(3_600),
        // Central Europe
        "CET" => Some(3_600),
        "CEST" => Some(7_200),
        // Eastern Europe
        "EET" => Some(7_200),
        "EEST" => Some(10_800),
        // Russia / Moscow
        "MSK" => Some(10_800),
        // North America – East
        "EST" => Some(-18_000),
        "EDT" => Some(-14_400),
        // North America – Central
        "CST" => Some(-21_600),
        "CDT" => Some(-18_000),
        // North America – Mountain
        "MST" => Some(-25_200),
        "MDT" => Some(-21_600),
        // North America – Pacific
        "PST" => Some(-28_800),
        "PDT" => Some(-25_200),
        // North America – Alaska
        "AKST" => Some(-32_400),
        "AKDT" => Some(-28_800),
        // North America – Hawaii
        "HST" => Some(-36_000),
        // Australia – East
        "AEST" => Some(36_000),
        "AEDT" => Some(39_600),
        // New Zealand
        "NZST" => Some(43_200),
        "NZDT" => Some(46_800),
        _ => None,
    }
}

/// Tries to parse strings in the format `DD Mon[.] YYYY HH:MM[:SS] [TZ]`
/// where `Mon` is a 3-letter (or full) English month name and `TZ` is an
/// optional well-known timezone abbreviation (see [`tz_abbrev_to_offset_secs`]).
///
/// Examples handled:
/// - `"18 Apr. 2026 15:00 CEST"` → 2026-04-18 15:00:00 +02:00
/// - `"18 Apr 2026 15:00 CET"`   → 2026-04-18 15:00:00 +01:00
/// - `"1 January 2024 08:00 UTC"` → 2024-01-01 08:00:00 UTC
/// - `"18 Apr. 2026 15:00:00 CEST"` → 2026-04-18 15:00:00 +02:00
/// - `"18 Apr. 2026 15:00"` → 2026-04-18 15:00:00 UTC (no tz → assumed UTC)
fn parse_day_mon_year_time_tz(s: &str) -> Option<OffsetDateTime> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    // Accepted: day month year time [tz]
    if parts.len() < 4 || parts.len() > 5 {
        return None;
    }

    let day: u8 = parts[0].parse().ok()?;
    let month_num = parse_month_abbrev(parts[1])?;
    let year: i32 = parts[2].parse().ok()?;

    // Time: "HH:MM" or "HH:MM:SS"
    let mut time_parts = parts[3].splitn(3, ':');
    let hour: u8 = time_parts.next()?.parse().ok()?;
    let minute: u8 = time_parts.next()?.parse().ok()?;
    let second: u8 = time_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let month = time::Month::try_from(month_num).ok()?;
    let date = time::Date::from_calendar_date(year, month, day).ok()?;
    let time = time::Time::from_hms(hour, minute, second).ok()?;
    let pdt = time::PrimitiveDateTime::new(date, time);

    if let Some(tz_str) = parts.get(4) {
        let offset_secs = tz_abbrev_to_offset_secs(tz_str)?;
        let offset = time::UtcOffset::from_whole_seconds(offset_secs).ok()?;
        Some(pdt.assume_offset(offset))
    } else {
        Some(pdt.assume_utc())
    }
}

/// Parses ISO 8601 basic (compact) datetime strings by expanding them into
/// extended RFC 3339 form.
///
/// Accepted patterns:
/// - `20240601T100000Z`
/// - `20240601T100000+0200`
/// - `20240601T100000+02:00`
/// - `20240601T100000.123Z`          (fractional seconds)
/// - `20240601T1000`                 (hours + minutes only, no offset → UTC)
fn parse_iso_basic(s: &str) -> Option<OffsetDateTime> {
    // Must start with 8 digits (YYYYMMDD) followed by T or t.
    let bytes = s.as_bytes();
    if bytes.len() < 9 {
        return None;
    }
    if !bytes[..8].iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if bytes[8] != b'T' && bytes[8] != b't' {
        return None;
    }

    let date_str = &s[..8];
    let after_t = &s[9..];

    // Extract date components.
    let year = &date_str[..4];
    let month = &date_str[4..6];
    let day = &date_str[6..8];

    // Parse time digits: expect at least HHMMSS or HHMM.
    let time_digits: String = after_t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if time_digits.len() < 4 {
        return None;
    }
    let hh = &time_digits[..2];
    let mm = &time_digits[2..4];
    let ss = if time_digits.len() >= 6 {
        &time_digits[4..6]
    } else {
        "00"
    };

    // Remainder after the consumed time digits (may start with . for fractions, then offset).
    let after_digits = &after_t[time_digits.len()..];

    // Strip optional fractional seconds so we can feed clean input to try_rfc3339.
    let after_frac = if let Some(rest) = after_digits.strip_prefix('.') {
        let frac_len = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        &after_digits[1 + frac_len..]
    } else {
        after_digits
    };

    // Determine offset string: Z, +HH:MM, +HHMM, or nothing (assume UTC).
    let offset = if after_frac.is_empty() || after_frac.eq_ignore_ascii_case("z") {
        "Z".to_owned()
    } else {
        // Could be +0200 or +02:00; normalise to +02:00.
        let sign = after_frac.chars().next()?;
        if sign != '+' && sign != '-' {
            return None;
        }
        let rest = &after_frac[1..];
        match rest.len() {
            4 if !rest.contains(':') => {
                format!("{}{}:{}", sign, &rest[..2], &rest[2..])
            }
            5 if rest.contains(':') => {
                format!("{}{}", sign, rest)
            }
            _ => return None,
        }
    };

    let extended = format!("{}-{}-{}T{}:{}:{}{}", year, month, day, hh, mm, ss, offset);
    try_rfc3339(&extended)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("date-time input could not be parsed")]
pub struct DateTimeNormalizationError;

/// Parses optional generic date-time input. Blank input means no assertion.
pub fn normalize_date_time(
    raw: Option<&str>,
) -> Result<Option<OffsetDateTime>, DateTimeNormalizationError> {
    let Some(raw) = raw else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    parse_datetime(trimmed)
        .map(Some)
        .ok_or(DateTimeNormalizationError)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::OffsetDateTime;
    use time::macros::datetime;

    use super::{
        DateTimeNormalizationError, fix_compact_offset, fix_hyphen_time_separators,
        normalize_date_time, parse_datetime, parse_month_abbrev, strip_fractional_seconds,
        tz_abbrev_to_offset_secs,
    };

    // -----------------------------------------------------------------------
    // Unit tests for internal helpers
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2026-03-03T07-30-00Z",      Some("2026-03-03T07:30:00Z".to_owned()))]
    #[case("2026-03-03T07-30-00+02:00", Some("2026-03-03T07:30:00+02:00".to_owned()))]
    #[case("2026-03-03T07-30-00+02-00", Some("2026-03-03T07:30:00+02-00".to_owned()))]
    #[case("2026-03-03T07:30:00Z", None)] // already valid → no fix needed
    fn should_fix_hyphen_time_separators_when_malformed_time_provided(
        #[case] input: &str,
        #[case] expected: Option<String>,
    ) {
        assert_eq!(fix_hyphen_time_separators(input), expected);
    }

    #[rstest]
    #[case("2024-06-01T10:00:00.123Z",   Some("2024-06-01T10:00:00Z".to_owned()))]
    #[case("2024-06-01T10:00:00.123456Z", Some("2024-06-01T10:00:00Z".to_owned()))]
    #[case("2024-06-01T10:00:00.1+02:00", Some("2024-06-01T10:00:00+02:00".to_owned()))]
    #[case("2024-06-01T10:00:00Z", None)] // no dot → no change
    fn should_strip_fractional_seconds_when_present(
        #[case] input: &str,
        #[case] expected: Option<String>,
    ) {
        assert_eq!(strip_fractional_seconds(input), expected);
    }

    #[rstest]
    #[case("2024-06-01T10:00:00+0200",  Some("2024-06-01T10:00:00+02:00".to_owned()))]
    #[case("2024-06-01T10:00:00-0500",  Some("2024-06-01T10:00:00-05:00".to_owned()))]
    #[case("2024-06-01T10:00:00+02:00", None)] // already has colon → no change
    #[case("2024-06-01T10:00:00Z", None)] // Z suffix → no change
    fn should_fix_compact_offset_when_no_colon_in_offset(
        #[case] input: &str,
        #[case] expected: Option<String>,
    ) {
        assert_eq!(fix_compact_offset(input), expected);
    }

    // -----------------------------------------------------------------------
    // RFC 3339 / ISO 8601 with explicit offset
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01T10:00:00+02:00", datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-06-01T10:00:00Z",      datetime!(2024-06-01 10:00:00 UTC))]
    #[case("2024-06-01T00:00:00Z",      datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-12-31T23:59:59Z",      datetime!(2024-12-31 23:59:59 UTC))]
    #[case("2024-06-01T10:00:00-05:00", datetime!(2024-06-01 10:00:00 -5))]
    fn should_parse_datetime_when_rfc3339_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // Fractional seconds
    // -----------------------------------------------------------------------

    // The `time` crate's Rfc3339 parser preserves sub-second precision, so the
    // parsed value will have non-zero nanoseconds. We compare unix timestamps
    // (whole seconds) and UTC offsets instead of full OffsetDateTime equality
    // to avoid the datetime! macro's zero-nanosecond mismatch.
    #[rstest]
    #[case("2024-06-01T10:00:00.0Z",       datetime!(2024-06-01 10:00:00 UTC).unix_timestamp(), 0)]
    #[case("2024-06-01T10:00:00.123Z",      datetime!(2024-06-01 10:00:00 UTC).unix_timestamp(), 0)]
    #[case("2024-06-01T10:00:00.999999Z",   datetime!(2024-06-01 10:00:00 UTC).unix_timestamp(), 0)]
    #[case("2024-06-01T10:00:00.500+02:00", datetime!(2024-06-01 10:00:00 +2).unix_timestamp(),  2 * 3600)]
    fn should_parse_datetime_when_fractional_seconds_provided(
        #[case] raw: &str,
        #[case] expected_unix: i64,
        #[case] expected_offset_secs: i32,
    ) {
        let dt = parse_datetime(raw).expect("should parse");
        assert_eq!(
            dt.unix_timestamp(),
            expected_unix,
            "unix timestamp mismatch for '{}'",
            raw
        );
        assert_eq!(
            dt.offset().whole_seconds(),
            expected_offset_secs,
            "offset mismatch for '{}'",
            raw
        );
    }

    // -----------------------------------------------------------------------
    // Compact offset (+0200 without colon)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01T10:00:00+0200", datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-06-01T10:00:00-0500", datetime!(2024-06-01 10:00:00 -5))]
    #[case("2024-06-01T10:00:00+0000", datetime!(2024-06-01 10:00:00 UTC))]
    fn should_parse_datetime_when_compact_offset_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // ISO 8601-like with space instead of T
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01 10:00:00+02:00", datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-06-01 10:00:00Z",      datetime!(2024-06-01 10:00:00 UTC))]
    fn should_parse_datetime_when_space_separated_iso8601_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // Space separator + "UTC" literal suffix
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01 10:00:00 UTC", datetime!(2024-06-01 10:00:00 UTC))]
    #[case("2024-12-31 23:59:59 UTC", datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_utc_suffix_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // Hyphenated time parts (the reported bug case)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2026-03-03T07-30-00Z",      datetime!(2026-03-03 07:30:00 UTC))]
    #[case("2024-06-01T10-00-00Z",      datetime!(2024-06-01 10:00:00 UTC))]
    #[case("2024-06-01T10-00-00+02:00", datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-06-01T10-00-00+0200",  datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-12-31T23-59-59Z",      datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_hyphenated_time_parts_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // ISO 8601 basic (compact) format
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("20240601T100000Z",     datetime!(2024-06-01 10:00:00 UTC))]
    #[case("20241231T235959Z",     datetime!(2024-12-31 23:59:59 UTC))]
    #[case("20240601T100000+0200", datetime!(2024-06-01 10:00:00 +2))]
    #[case("20240601T100000+02:00", datetime!(2024-06-01 10:00:00 +2))]
    #[case("20240601T100000.123Z", datetime!(2024-06-01 10:00:00 UTC))]
    fn should_parse_datetime_when_iso_basic_format_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // "DD Mon[.] YYYY HH:MM[:SS] [TZ]" – English month names + tz abbrev
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("Jan", Some(1))]
    #[case("Jan.", Some(1))]
    #[case("january", Some(1))]
    #[case("apr", Some(4))]
    #[case("Apr.", Some(4))]
    #[case("April", Some(4))]
    #[case("sep", Some(9))]
    #[case("Sept.", Some(9))]
    #[case("September", Some(9))]
    #[case("dec", Some(12))]
    #[case("December", Some(12))]
    #[case("xyz", None)]
    fn should_return_month_number_when_month_abbrev_provided(
        #[case] input: &str,
        #[case] expected: Option<u8>,
    ) {
        assert_eq!(parse_month_abbrev(input), expected);
    }

    #[rstest]
    #[case("UTC", Some(0))]
    #[case("GMT", Some(0))]
    #[case("utc", Some(0))]
    #[case("CET", Some(3_600))]
    #[case("CEST", Some(7_200))]
    #[case("BST", Some(3_600))]
    #[case("EET", Some(7_200))]
    #[case("EEST", Some(10_800))]
    #[case("EST",  Some(-18_000))]
    #[case("EDT",  Some(-14_400))]
    #[case("PST",  Some(-28_800))]
    #[case("PDT",  Some(-25_200))]
    #[case("AEST", Some(36_000))]
    #[case("NZST", Some(43_200))]
    #[case("UNKNOWN", None)]
    fn should_return_offset_secs_when_tz_abbrev_provided(
        #[case] tz: &str,
        #[case] expected: Option<i32>,
    ) {
        assert_eq!(tz_abbrev_to_offset_secs(tz), expected);
    }

    // The primary failing case from the issue report.
    #[test]
    fn should_parse_datetime_when_day_month_abbrev_year_time_cest_provided() {
        assert_eq!(
            parse_datetime("18 Apr. 2026 15:00 CEST"),
            Some(datetime!(2026-04-18 15:00:00 +2))
        );
    }

    #[rstest]
    // With period after month abbreviation
    #[case("18 Apr. 2026 15:00 CEST",  datetime!(2026-04-18 15:00:00 +2))]
    // Without period
    #[case("18 Apr 2026 15:00 CET",    datetime!(2026-04-18 15:00:00 +1))]
    // Full month name
    #[case("1 January 2024 08:00 UTC", datetime!(2024-01-01 08:00:00 UTC))]
    // GMT
    #[case("31 Dec 2024 23:59 GMT",    datetime!(2024-12-31 23:59:00 UTC))]
    // BST (British Summer Time = +01:00)
    #[case("15 Jun. 2025 10:00 BST",   datetime!(2025-06-15 10:00:00 +1))]
    // EEST (Eastern European Summer Time = +03:00)
    #[case("20 Jul. 2024 12:00 EEST",  datetime!(2024-07-20 12:00:00 +3))]
    // EST (Eastern Standard Time = -05:00)
    #[case("5 Nov 2024 09:00 EST",     datetime!(2024-11-05 09:00:00 -5))]
    // PST (Pacific Standard Time = -08:00)
    #[case("1 Feb. 2025 08:30 PST",    datetime!(2025-02-01 08:30:00 -8))]
    // With seconds
    #[case("18 Apr. 2026 15:00:30 CEST", datetime!(2026-04-18 15:00:30 +2))]
    // No timezone → assumed UTC
    #[case("18 Apr. 2026 15:00",       datetime!(2026-04-18 15:00:00 UTC))]
    fn should_parse_datetime_when_day_mon_year_time_tz_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected), "failed for '{raw}'");
    }

    // -----------------------------------------------------------------------
    // ISO 8601 date-only (→ midnight UTC, no tz info present)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01",  datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-12-31",  datetime!(2024-12-31 00:00:00 UTC))]
    #[case("2000-01-01",  datetime!(2000-01-01 00:00:00 UTC))]
    fn should_parse_datetime_when_iso_date_only_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // SQL-style (YYYY-MM-DD HH:MM[:SS], no timezone → UTC)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01 10:30:00",  datetime!(2024-06-01 10:30:00 UTC))]
    #[case("2024-06-01 00:00:00",  datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-12-31 23:59:59",  datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_sql_datetime_with_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("2024-06-01 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("2024-12-31 23:59", datetime!(2024-12-31 23:59:00 UTC))]
    fn should_parse_datetime_when_sql_datetime_without_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // German / European format (DD.MM.YYYY)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("01.06.2024 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("31.12.2024 23:59:59", datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_german_datetime_with_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("01.06.2024 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("31.12.2024 23:59", datetime!(2024-12-31 23:59:00 UTC))]
    fn should_parse_datetime_when_german_datetime_without_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("01.06.2024", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("31.12.2024", datetime!(2024-12-31 00:00:00 UTC))]
    fn should_parse_datetime_when_german_date_only_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // US format (MM/DD/YYYY)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("06/01/2024 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("12/31/2024 23:59:59", datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_us_datetime_with_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("06/01/2024 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("12/31/2024 23:59", datetime!(2024-12-31 23:59:00 UTC))]
    fn should_parse_datetime_when_us_datetime_without_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("06/01/2024", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("12/31/2024", datetime!(2024-12-31 00:00:00 UTC))]
    fn should_parse_datetime_when_us_date_only_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // Unix epoch
    // -----------------------------------------------------------------------

    #[test]
    fn should_parse_datetime_when_unix_epoch_string_provided() {
        let result = parse_datetime("1717228800");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unix_timestamp(), 1717228800);
    }

    #[test]
    fn should_parse_datetime_when_zero_unix_epoch_provided() {
        let result = parse_datetime("0");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unix_timestamp(), 0);
    }

    #[test]
    fn should_parse_datetime_when_negative_unix_epoch_provided() {
        let result = parse_datetime("-86400");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unix_timestamp(), -86400);
    }

    // -----------------------------------------------------------------------
    // Whitespace handling
    // -----------------------------------------------------------------------

    #[test]
    fn should_trim_whitespace_before_parsing() {
        assert_eq!(
            parse_datetime("  2024-06-01T10:00:00Z  "),
            Some(datetime!(2024-06-01 10:00:00 UTC))
        );
    }

    // -----------------------------------------------------------------------
    // Unparseable inputs
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("not a date")]
    #[case("32.13.2024")]
    #[case("2024-13-01")]
    #[case("yesterday at noon")]
    #[case("next tuesday")]
    fn should_return_none_when_datetime_string_is_unparseable(#[case] raw: &str) {
        assert_eq!(parse_datetime(raw), None, "expected None for '{}'", raw);
    }

    #[test]
    fn should_return_none_when_input_is_empty() {
        assert_eq!(parse_datetime(""), None);
    }

    // -----------------------------------------------------------------------
    // normalize_date_time
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_none_when_optional_datetime_is_absent_or_blank() {
        assert_eq!(normalize_date_time(None), Ok(None));
        assert_eq!(normalize_date_time(Some("  ")), Ok(None));
    }

    #[test]
    fn should_return_typed_error_when_optional_datetime_is_invalid() {
        assert!(matches!(
            normalize_date_time(Some("not a date")),
            Err(DateTimeNormalizationError)
        ));
    }
}
