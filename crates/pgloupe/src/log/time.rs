//! Parsing of Postgres log timestamps and timezone tokens.

use std::cell::RefCell;

use jiff::{Timestamp, civil::DateTime, tz::Offset};

/// Parses `YYYY-MM-DD HH:MM:SS[.fff] ZONE`, e.g. `2026-10-14 10:20:09.289 UTC`.
///
/// `fallback` applies only to alphabetic abbreviations other than `UTC`, `GMT` and `Z`.
///
/// # Errors
///
/// Fails if the text is not a date and time followed by a zone, or the zone is unknown.
pub fn parse_log_time(text: &str, fallback: Option<Offset>) -> Result<Timestamp, String> {
    if let Some(found) = LAST_SECOND.with(|last| last.borrow().as_ref()?.resolve(text, fallback)) {
        return Ok(found);
    }
    let parsed = parse_uncached(text, fallback)?;
    if let Some((head, zone)) = split_log_time(text) {
        // The same second without its fraction: what the next line of this second starts from.
        if let Ok(base) = parse_uncached(&format!("{head} {zone}"), fallback) {
            LAST_SECOND.with(|last| {
                *last.borrow_mut() = Some(Second {
                    head: head.to_owned(),
                    zone: zone.to_owned(),
                    fallback,
                    base,
                });
            });
        }
    }
    Ok(parsed)
}

fn parse_uncached(text: &str, fallback: Option<Offset>) -> Result<Timestamp, String> {
    let (civil, zone) = text
        .rsplit_once(' ')
        .ok_or_else(|| format!("{text:?} has no timezone"))?;
    let datetime: DateTime = civil
        .parse()
        .map_err(|err| format!("{civil:?} is not a date and time: {err}"))?;
    let offset = zone_offset(zone, fallback)?;
    offset
        .to_timestamp(datetime)
        .map_err(|err| format!("{text:?} is out of range: {err}"))
}

/// The second the last line was logged in. Lines come in bursts, and most share the second of the
/// one before, so only the fraction of such a line needs reading.
struct Second {
    /// `YYYY-MM-DD HH:MM:SS`
    head: String,
    zone: String,
    fallback: Option<Offset>,
    /// The instant of `head` in `zone`.
    base: Timestamp,
}

thread_local! {
    static LAST_SECOND: RefCell<Option<Second>> = const { RefCell::new(None) };
}

/// `text` as (`YYYY-MM-DD HH:MM:SS`, zone) when it has no fraction or a plain one of 1 to 9 digits.
fn split_log_time(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let head = text.get(..19)?;
    let after = match bytes.get(19)? {
        b' ' => 20,
        b'.' => {
            let digits = bytes[20..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            (1..=9).contains(&digits).then_some(())?;
            if bytes.get(20 + digits) != Some(&b' ') {
                return None;
            }
            21 + digits
        }
        _ => return None,
    };
    let zone = text.get(after..)?;
    (!zone.is_empty() && !zone.contains(' ')).then_some((head, zone))
}

impl Second {
    fn resolve(&self, text: &str, fallback: Option<Offset>) -> Option<Timestamp> {
        let (head, zone) = split_log_time(text)?;
        if head != self.head || zone != self.zone || fallback != self.fallback {
            return None;
        }
        let fraction = text[19..].strip_prefix('.').map_or("", |rest| {
            &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()]
        });
        let mut nanos: i32 = 0;
        for place in 0..9 {
            let digit = fraction
                .as_bytes()
                .get(place)
                .map_or(0, |b| i32::from(b - b'0'));
            nanos = nanos * 10 + digit;
        }
        Timestamp::new(self.base.as_second(), nanos).ok()
    }
}

/// Parses epoch seconds with an optional fraction, as printed by `%n`.
///
/// # Errors
///
/// Fails if the text is not `seconds[.fraction]`.
pub fn parse_epoch(text: &str) -> Result<Timestamp, String> {
    let (seconds, fraction) = text.split_once('.').unwrap_or((text, ""));
    let seconds: i64 = seconds
        .parse()
        .map_err(|_err| format!("{text:?} is not epoch seconds"))?;
    let digits: String = fraction.chars().take(9).collect();
    let nanos: i32 = if digits.is_empty() {
        0
    } else {
        format!("{digits:0<9}")
            .parse()
            .map_err(|_err| format!("{text:?} is not epoch seconds"))?
    };
    Timestamp::new(seconds, nanos).map_err(|err| format!("{text:?} is out of range: {err}"))
}

/// Parses the `--log-tz` argument: `UTC` or `[+-]HH`, `[+-]HHMM`, `[+-]HH:MM`.
///
/// # Errors
///
/// Fails with a message that shows the accepted spellings.
pub fn parse_offset_arg(text: &str) -> Result<Offset, String> {
    known_zone(text)
        .ok_or_else(|| format!("{text:?} is not UTC or an offset like +02, -0300 or +05:30"))
}

/// True if `text` is exactly a csvlog `log_time` field: date, time, zone and nothing else.
#[must_use]
pub fn looks_like_log_time(text: &str) -> bool {
    let mut parts = text.split(' ');
    let (Some(date), Some(time), Some(zone), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (date, time) = (date.as_bytes(), time.as_bytes());
    date.len() == 10
        && date.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
        && time.len() >= 8
        && time[..8].iter().enumerate().all(|(i, c)| {
            if i == 2 || i == 5 {
                *c == b':'
            } else {
                c.is_ascii_digit()
            }
        })
        && !zone.is_empty()
}

fn zone_offset(zone: &str, fallback: Option<Offset>) -> Result<Offset, String> {
    if let Some(offset) = known_zone(zone) {
        return Ok(offset);
    }
    if !zone.is_empty() && zone.bytes().all(|b| b.is_ascii_alphabetic()) {
        return fallback.ok_or_else(|| {
            format!("unknown timezone abbreviation '{zone}'; pass --log-tz +HH[:MM]")
        });
    }
    Err(format!("{zone:?} is not a timezone"))
}

/// `UTC`, `GMT`, `Z`, or a numeric offset.
fn known_zone(zone: &str) -> Option<Offset> {
    if matches!(zone, "UTC" | "GMT" | "Z") {
        return Some(Offset::UTC);
    }
    let sign: i32 = match zone.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let rest: String = zone[1..].chars().filter(|c| *c != ':').collect();
    if !rest.bytes().all(|b| b.is_ascii_digit()) || !matches!(rest.len(), 2 | 4) {
        return None;
    }
    let hours: i32 = rest[..2].parse().ok()?;
    let minutes: i32 = if rest.len() == 4 {
        rest[2..].parse().ok()?
    } else {
        0
    };
    Offset::from_seconds(sign * (hours * 3600 + minutes * 60)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn utc_gmt_and_z_are_zero_offset() {
        for zone in ["UTC", "GMT", "Z"] {
            assert_eq!(
                parse_log_time(&format!("2026-10-14 10:20:09.289 {zone}"), None),
                Ok(at("2026-10-14T10:20:09.289Z")),
                "{zone}"
            );
        }
    }

    #[test]
    fn numeric_offsets_in_all_three_spellings() {
        let expected = at("2026-10-14T13:20:09Z");
        assert_eq!(
            parse_log_time("2026-10-14 10:20:09 -03", None),
            Ok(expected)
        );
        assert_eq!(
            parse_log_time("2026-10-14 10:20:09 -0300", None),
            Ok(expected)
        );
        assert_eq!(
            parse_log_time("2026-10-14 10:20:09 -03:00", None),
            Ok(expected)
        );
        assert_eq!(
            parse_log_time("2026-10-14 10:20:09 +05:30", None),
            Ok(at("2026-10-14T04:50:09Z"))
        );
        assert_eq!(
            parse_log_time("2026-10-14 10:20:09 +00", None),
            Ok(at("2026-10-14T10:20:09Z"))
        );
    }

    #[test]
    fn unknown_abbreviation_needs_log_tz() {
        let err = parse_log_time("2026-10-14 10:20:09 BRT", None).unwrap_err();
        assert!(err.contains("unknown timezone abbreviation 'BRT'"), "{err}");
        assert!(err.contains("--log-tz"), "{err}");
        let minus_three = Offset::from_seconds(-3 * 3600).unwrap();
        assert_eq!(
            parse_log_time("2026-10-14 10:20:09 BRT", Some(minus_three)),
            Ok(at("2026-10-14T13:20:09Z"))
        );
        // The fallback never overrides a zone the log states itself.
        assert_eq!(
            parse_log_time("2026-10-14 10:20:09 UTC", Some(minus_three)),
            Ok(at("2026-10-14T10:20:09Z"))
        );
    }

    #[test]
    fn garbage_is_an_error() {
        parse_log_time("not a time", None).unwrap_err();
        parse_log_time("2026-10-14 10:20:09.289", None).unwrap_err();
        parse_log_time("2026-13-45 10:20:09 UTC", None).unwrap_err();
        parse_log_time("", None).unwrap_err();
    }

    #[test]
    fn epoch_seconds_with_and_without_fraction() {
        assert_eq!(
            parse_epoch("1760437209.289"),
            Ok(at("2025-10-14T10:20:09.289Z"))
        );
        assert_eq!(parse_epoch("1760437209"), Ok(at("2025-10-14T10:20:09Z")));
        parse_epoch("abc").unwrap_err();
    }

    #[test]
    fn log_tz_argument_grammar() {
        assert_eq!(parse_offset_arg("UTC"), Ok(Offset::UTC));
        assert_eq!(
            parse_offset_arg("-03"),
            Ok(Offset::from_seconds(-3 * 3600).unwrap())
        );
        assert_eq!(
            parse_offset_arg("+05:30"),
            Ok(Offset::from_seconds(5 * 3600 + 30 * 60).unwrap())
        );
        parse_offset_arg("BRT").unwrap_err();
        parse_offset_arg("").unwrap_err();
    }

    #[test]
    fn csvlog_first_field_is_recognized() {
        assert!(looks_like_log_time("2026-10-14 10:20:09.289 UTC"));
        assert!(looks_like_log_time("2026-10-14 10:20:09 -03"));
        assert!(!looks_like_log_time(
            "2026-10-14 10:20:09.289 UTC [42] LOG:  hi"
        ));
        assert!(!looks_like_log_time("hello"));
        assert!(!looks_like_log_time(""));
    }

    #[test]
    fn the_second_cache_agrees_with_a_full_parse() {
        let zones = ["UTC", "+02", "-0300", "+05:30", "GMT"];
        let fractions = ["", ".5", ".05", ".123", ".999999999", ".1234567890", ".x"];
        for fallback in [None, Some(Offset::UTC)] {
            for zone in zones {
                for second in [
                    "2026-10-14 10:20:09",
                    "2026-10-14 10:20:10",
                    "2026-12-31 23:59:59",
                ] {
                    for fraction in fractions {
                        for text in [
                            format!("{second}{fraction} {zone}"),
                            format!("{second}{fraction} {zone}"),
                        ] {
                            assert_eq!(
                                parse_log_time(&text, fallback),
                                parse_uncached(&text, fallback),
                                "{text}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_new_zone_or_fallback_is_not_served_from_the_last_second() {
        let utc = parse_log_time("2026-10-14 10:20:09.5 UTC", None).unwrap();
        let east = parse_log_time("2026-10-14 10:20:09.5 +02", None).unwrap();
        assert_ne!(utc, east);
        assert_eq!(east.as_second(), utc.as_second() - 7_200);
        parse_log_time("2026-10-14 10:20:09.5 CEST", None).unwrap_err();
        parse_log_time("2026-10-14 10:20:09.5 CEST", Some(Offset::UTC)).unwrap();
    }
}
