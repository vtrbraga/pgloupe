//! Choosing which records of a log take part in an analysis.

use regex::Regex;

use super::event::{Event, StatementKind, classify};
use crate::log::LogRecord;

/// One value to look for in an attribute: an exact name, a glob (`*`, `?`) or `re:PATTERN`.
#[derive(Debug, Clone)]
pub struct Matcher {
    source: String,
    kind: MatcherKind,
}

#[derive(Debug, Clone)]
enum MatcherKind {
    Exact,
    Pattern(Regex),
}

impl Matcher {
    /// An exact name, whatever characters it has.
    #[must_use]
    pub fn exact(name: &str) -> Self {
        Matcher {
            source: name.to_owned(),
            kind: MatcherKind::Exact,
        }
    }

    /// Reads `text` as `re:PATTERN`, a glob when it holds `*` or `?`, or an exact name.
    ///
    /// # Errors
    ///
    /// Fails with the regex error when a `re:` pattern does not compile.
    pub fn parse(text: &str) -> Result<Self, String> {
        let kind = if let Some(pattern) = text.strip_prefix("re:") {
            MatcherKind::Pattern(Regex::new(pattern).map_err(|e| format!("{text}: {e}"))?)
        } else if text.contains(['*', '?']) {
            let mut regex = String::from("^");
            for c in text.chars() {
                match c {
                    '*' => regex.push_str(".*"),
                    '?' => regex.push('.'),
                    other => regex.push_str(&regex::escape(&other.to_string())),
                }
            }
            regex.push('$');
            MatcherKind::Pattern(Regex::new(&regex).map_err(|e| format!("{text}: {e}"))?)
        } else {
            MatcherKind::Exact
        };
        Ok(Matcher {
            source: text.to_owned(),
            kind,
        })
    }

    fn matches(&self, value: &str) -> bool {
        match &self.kind {
            MatcherKind::Exact => self.source == value,
            MatcherKind::Pattern(regex) => regex.is_match(value),
        }
    }
}

/// Values a record must (include) or must not (exclude) carry, per attribute.
#[derive(Debug, Clone, Default)]
pub struct Facets {
    pub databases: Vec<Matcher>,
    pub users: Vec<Matcher>,
    pub applications: Vec<Matcher>,
    pub hosts: Vec<Matcher>,
}

impl Facets {
    fn is_empty(&self) -> bool {
        self.databases.is_empty()
            && self.users.is_empty()
            && self.applications.is_empty()
            && self.hosts.is_empty()
    }

    fn pairs<'a>(&'a self, record: &'a LogRecord) -> [(&'a [Matcher], Option<&'a str>); 4] {
        [
            (&self.databases, record.database.as_deref()),
            (&self.users, record.user.as_deref()),
            (&self.applications, record.application.as_deref()),
            (&self.hosts, record.host.as_deref()),
        ]
    }

    fn describe(&self, prefix: &str, out: &mut Vec<String>) {
        for (name, list) in [
            ("database", &self.databases),
            ("user", &self.users),
            ("application", &self.applications),
            ("host", &self.hosts),
        ] {
            if !list.is_empty() {
                let values: Vec<&str> = list.iter().map(|m| m.source.as_str()).collect();
                out.push(format!("{prefix}{name}={}", values.join(",")));
            }
        }
    }
}

/// When a record happened, in a time zone: hours of the day to keep or skip, and days of the week.
#[derive(Debug, Clone, Default)]
pub struct Clock {
    /// Zone the hours and days are read in (UTC when `None`).
    pub zone: Option<jiff::tz::TimeZone>,
    /// What the zone was called on the command line.
    pub zone_name: Option<String>,
    /// Keep only these minutes of the day: `(from, to)`, `to` exclusive; wraps past midnight when
    /// `from > to`.
    pub between: Option<(u16, u16)>,
    /// Drop these hours (0-23), as `from..=to` ranges.
    pub exclude_hours: Vec<(u8, u8)>,
    /// Keep only these days (Monday is 0).
    pub weekdays: Option<[bool; 7]>,
    source: Vec<String>,
}

impl Clock {
    /// `HH:MM-HH:MM`.
    ///
    /// # Errors
    ///
    /// Fails when either end is not a time of day.
    pub fn parse_between(text: &str) -> Result<(u16, u16), String> {
        let (from, to) = text
            .split_once('-')
            .ok_or_else(|| format!("{text}: use HH:MM-HH:MM"))?;
        Ok((minutes(from)?, minutes(to)?))
    }

    /// `5-7` or `5`: hours of the day.
    ///
    /// # Errors
    ///
    /// Fails when the range is not made of hours 0-23.
    pub fn parse_hours(text: &str) -> Result<(u8, u8), String> {
        let (from, to) = text.split_once('-').unwrap_or((text, text));
        let hour = |t: &str| {
            t.trim()
                .parse::<u8>()
                .ok()
                .filter(|h| *h < 24)
                .ok_or_else(|| format!("{text}: hours are 0-23, as 5 or 5-7"))
        };
        Ok((hour(from)?, hour(to)?))
    }

    /// `mon-fri`, `sat,sun`, `mon,wed-fri`.
    ///
    /// # Errors
    ///
    /// Fails on a day name that is not `mon`..`sun`.
    pub fn parse_weekdays(text: &str) -> Result<[bool; 7], String> {
        const NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
        let day = |t: &str| {
            let t = t.trim().to_ascii_lowercase();
            NAMES
                .iter()
                .position(|n| t.starts_with(n))
                .ok_or_else(|| format!("{text}: days are mon..sun"))
        };
        let mut days = [false; 7];
        for part in text.split(',') {
            let (from, to) = part.split_once('-').unwrap_or((part, part));
            let (from, to) = (day(from)?, day(to)?);
            let mut d = from;
            loop {
                days[d] = true;
                if d == to {
                    break;
                }
                d = (d + 1) % 7;
            }
        }
        Ok(days)
    }

    /// A clock from the command-line values; keeps them as written for [`Filter::describe`].
    #[must_use]
    pub fn new(
        between: Option<(u16, u16)>,
        exclude_hours: Vec<(u8, u8)>,
        weekdays: Option<[bool; 7]>,
        zone: Option<(jiff::tz::TimeZone, String)>,
    ) -> Self {
        let mut source = Vec::new();
        if let Some((from, to)) = between {
            source.push(format!(
                "between={:02}:{:02}-{:02}:{:02}",
                from / 60,
                from % 60,
                to / 60,
                to % 60
            ));
        }
        if !exclude_hours.is_empty() {
            let hours: Vec<String> = exclude_hours
                .iter()
                .map(|(a, b)| format!("{a}-{b}"))
                .collect();
            source.push(format!("exclude-hours={}", hours.join(",")));
        }
        if let Some(days) = weekdays {
            const NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
            let picked: Vec<&str> = NAMES
                .iter()
                .zip(days)
                .filter_map(|(n, on)| on.then_some(*n))
                .collect();
            source.push(format!("weekdays={}", picked.join(",")));
        }
        let (zone, zone_name) = zone.map_or((None, None), |(z, n)| (Some(z), Some(n)));
        Clock {
            zone,
            zone_name,
            between,
            exclude_hours,
            weekdays,
            source,
        }
    }

    fn is_empty(&self) -> bool {
        self.between.is_none() && self.exclude_hours.is_empty() && self.weekdays.is_none()
    }

    fn keeps(&self, ts: jiff::Timestamp) -> bool {
        if self.is_empty() {
            return true;
        }
        let zone = self.zone.clone().unwrap_or(jiff::tz::TimeZone::UTC);
        let local = ts.to_zoned(zone);
        if let Some(days) = self.weekdays {
            let day = usize::from(local.weekday().to_monday_zero_offset().unsigned_abs());
            if !days[day] {
                return false;
            }
        }
        let hour = u8::try_from(local.hour()).unwrap_or(0);
        if self
            .exclude_hours
            .iter()
            .any(|(a, b)| (*a..=*b).contains(&hour))
        {
            return false;
        }
        if let Some((from, to)) = self.between {
            let now = u16::try_from(i32::from(local.hour()) * 60 + i32::from(local.minute()))
                .unwrap_or(0);
            let inside = if from <= to {
                (from..to).contains(&now)
            } else {
                now >= from || now < to
            };
            if !inside {
                return false;
            }
        }
        true
    }
}

fn minutes(text: &str) -> Result<u16, String> {
    let (h, m) = text
        .trim()
        .split_once(':')
        .ok_or_else(|| format!("{text}: use HH:MM"))?;
    let (h, m): (u16, u16) = (
        h.parse()
            .map_err(|_unparsed| format!("{text}: use HH:MM"))?,
        m.parse()
            .map_err(|_unparsed| format!("{text}: use HH:MM"))?,
    );
    if h > 24 || m > 59 || (h == 24 && m > 0) {
        return Err(format!("{text}: not a time of day"));
    }
    Ok(h * 60 + m)
}

/// Which records are kept: every include list that is set must contain the record's value, no
/// exclude list may, and the statement patterns must agree.
///
/// The statement patterns judge only statement records (`statement:` and `execute`); every other
/// record, including bare `duration:` lines and the errors or lock waits of a dropped statement,
/// is kept, because it cannot be tied to the statement's text.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub include: Facets,
    pub exclude: Facets,
    /// Keep only statements whose text matches.
    pub include_query: Option<Regex>,
    /// Drop statements whose text matches.
    pub exclude_query: Option<Regex>,
    /// Keep only these backend process ids.
    pub pids: Vec<u32>,
    /// Drop records whose message matches.
    pub exclude_message: Option<Regex>,
    /// Time of day and day of week.
    pub clock: Clock,
    /// Anonymize records as they are read, with this key (see [`super::anonymize`]).
    pub anonymize: Option<String>,
}

impl Filter {
    /// Whether the filter keeps everything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.include.is_empty()
            && self.exclude.is_empty()
            && self.include_query.is_none()
            && self.exclude_query.is_none()
            && self.pids.is_empty()
            && self.exclude_message.is_none()
            && self.clock.is_empty()
            && self.anonymize.is_none()
    }

    /// The filter as one line, empty when it keeps everything. A store keeps this next to what
    /// it ingested, so a report can say what the numbers leave out.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        self.include.describe("", &mut parts);
        self.exclude.describe("exclude-", &mut parts);
        if let Some(r) = &self.include_query {
            parts.push(format!("include-query={r}"));
        }
        if let Some(r) = &self.exclude_query {
            parts.push(format!("exclude-query={r}"));
        }
        if !self.pids.is_empty() {
            let pids: Vec<String> = self.pids.iter().map(u32::to_string).collect();
            parts.push(format!("pid={}", pids.join(",")));
        }
        if let Some(r) = &self.exclude_message {
            parts.push(format!("exclude-message={r}"));
        }
        parts.extend(self.clock.source.iter().cloned());
        if !self.clock.is_empty()
            && let Some(name) = &self.clock.zone_name
        {
            parts.push(format!("tz={name}"));
        }
        if self.anonymize.is_some() {
            parts.push("anonymize".to_owned());
        }
        parts.join("; ")
    }

    /// Whether `record` takes part in the analysis. A record without an attribute fails an include
    /// list for it and passes an exclude list.
    #[must_use]
    pub fn keeps(&self, record: &LogRecord) -> bool {
        for (wanted, actual) in self.include.pairs(record) {
            if !wanted.is_empty()
                && !actual.is_some_and(|value| wanted.iter().any(|w| w.matches(value)))
            {
                return false;
            }
        }
        for (unwanted, actual) in self.exclude.pairs(record) {
            if actual.is_some_and(|value| unwanted.iter().any(|u| u.matches(value))) {
                return false;
            }
        }
        if !self.pids.is_empty() && !record.pid.is_some_and(|p| self.pids.contains(&p)) {
            return false;
        }
        if self
            .exclude_message
            .as_ref()
            .is_some_and(|r| r.is_match(&record.message))
        {
            return false;
        }
        if !self.clock.keeps(record.ts) {
            return false;
        }
        let judges_statements = self.include_query.is_some() || self.exclude_query.is_some();
        if judges_statements
            && let Event::Statement {
                kind: StatementKind::Statement | StatementKind::Execute,
                text,
                ..
            } = classify(record)
        {
            if let Some(pattern) = &self.include_query
                && !pattern.is_match(&text)
            {
                return false;
            }
            if let Some(pattern) = &self.exclude_query
                && pattern.is_match(&text)
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::log::Severity;

    fn record(database: Option<&str>, user: Option<&str>, message: &str) -> LogRecord {
        LogRecord {
            database: database.map(str::to_owned),
            user: user.map(str::to_owned),
            ..LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, message)
        }
    }

    fn names(values: &[&str]) -> Vec<Matcher> {
        values.iter().map(|v| Matcher::parse(v).unwrap()).collect()
    }

    #[test]
    fn an_empty_filter_keeps_everything() {
        let filter = Filter::default();
        assert!(filter.is_empty());
        assert!(filter.keeps(&record(None, None, "anything")));
    }

    #[test]
    fn an_include_list_needs_the_attribute_to_match() {
        let filter = Filter {
            include: Facets {
                databases: names(&["shop", "crm"]),
                ..Facets::default()
            },
            ..Filter::default()
        };
        assert!(filter.keeps(&record(Some("shop"), None, "x")));
        assert!(!filter.keeps(&record(Some("other"), None, "x")));
        assert!(
            !filter.keeps(&record(None, Some("app"), "x")),
            "a record without the attribute fails an include list"
        );
    }

    #[test]
    fn an_exclude_list_drops_matches_and_keeps_records_without_the_attribute() {
        let filter = Filter {
            exclude: Facets {
                users: names(&["monitor"]),
                ..Facets::default()
            },
            ..Filter::default()
        };
        assert!(!filter.keeps(&record(None, Some("monitor"), "x")));
        assert!(filter.keeps(&record(None, Some("app"), "x")));
        assert!(filter.keeps(&record(None, None, "x")));
    }

    #[test]
    fn include_and_exclude_of_the_same_value_leaves_nothing() {
        let both = Facets {
            databases: names(&["shop"]),
            ..Facets::default()
        };
        let filter = Filter {
            include: both.clone(),
            exclude: both,
            ..Filter::default()
        };
        assert!(!filter.keeps(&record(Some("shop"), None, "x")));
        assert!(!filter.keeps(&record(Some("crm"), None, "x")));
    }

    #[test]
    fn statement_patterns_look_at_the_message_and_the_attached_statement() {
        let filter = Filter {
            include_query: Regex::new("(?i)from film").ok(),
            exclude_query: Regex::new("pg_sleep").ok(),
            ..Filter::default()
        };
        assert!(filter.keeps(&record(
            None,
            None,
            "duration: 1 ms  statement: SELECT * FROM film"
        )));
        assert!(!filter.keeps(&record(None, None, "duration: 1 ms  statement: SELECT 1")));
        assert!(!filter.keeps(&record(
            None,
            None,
            "statement: SELECT pg_sleep(1) FROM film"
        )));
        let mut error = record(None, None, "canceling statement due to user request");
        error.statement = Some("SELECT * FROM film".to_owned());
        assert!(filter.keeps(&error));
    }

    fn with_app(app: &str) -> LogRecord {
        LogRecord {
            application: Some(app.to_owned()),
            ..record(None, None, "x")
        }
    }

    #[test]
    fn globs_and_regexes_match_names_and_plain_text_is_exact() {
        let filter = Filter {
            exclude: Facets {
                applications: names(&["pgAdmin 4*", "re:^DBeaver [0-9.]+ - ", "psql"]),
                ..Facets::default()
            },
            ..Filter::default()
        };
        assert!(!filter.keeps(&with_app("pgAdmin 4 - CONN:123")));
        assert!(!filter.keeps(&with_app("DBeaver 24.1 - Main")));
        assert!(!filter.keeps(&with_app("psql")));
        assert!(filter.keeps(&with_app("psql2")), "plain text is exact");
        assert!(filter.keeps(&with_app("app-web-1")));
        Matcher::parse("re:(").unwrap_err();
        assert!(Matcher::parse("a.b*").unwrap().matches("a.bzz"));
        assert!(
            !Matcher::parse("a.b*").unwrap().matches("axbzz"),
            "dots in globs are literal"
        );
        assert!(Matcher::parse("?x").unwrap().matches("ax"));
    }

    #[test]
    fn pid_and_message_filters() {
        let mut r = record(None, None, "connection authorized: user=monitor");
        r.pid = Some(7);
        let by_pid = Filter {
            pids: vec![7],
            ..Filter::default()
        };
        assert!(by_pid.keeps(&r));
        r.pid = Some(8);
        assert!(!by_pid.keeps(&r));
        r.pid = None;
        assert!(!by_pid.keeps(&r));
        let no_noise = Filter {
            exclude_message: Regex::new("user=monitor").ok(),
            ..Filter::default()
        };
        assert!(!no_noise.keeps(&record(None, None, "connection authorized: user=monitor")));
        assert!(no_noise.keeps(&record(None, None, "connection authorized: user=app")));
    }

    fn at(text: &str) -> LogRecord {
        LogRecord::new(text.parse().unwrap(), Severity::Log, "x")
    }

    fn clock(f: impl FnOnce(&mut Clock)) -> Filter {
        let mut clock = Clock::default();
        f(&mut clock);
        Filter {
            clock,
            ..Filter::default()
        }
    }

    #[test]
    fn working_hours_and_excluded_hours_use_the_clock_of_the_zone() {
        let day = clock(|c| c.between = Some(Clock::parse_between("09:00-18:00").unwrap()));
        assert!(day.keeps(&at("2026-10-14T09:00:00Z")));
        assert!(day.keeps(&at("2026-10-14T17:59:59Z")));
        assert!(!day.keeps(&at("2026-10-14T18:00:00Z")));
        let night = clock(|c| c.between = Some(Clock::parse_between("22:00-06:00").unwrap()));
        assert!(night.keeps(&at("2026-10-14T23:30:00Z")));
        assert!(night.keeps(&at("2026-10-15T05:59:00Z")));
        assert!(!night.keeps(&at("2026-10-15T06:00:00Z")));
        let batch = clock(|c| c.exclude_hours = vec![Clock::parse_hours("5-7").unwrap()]);
        assert!(!batch.keeps(&at("2026-10-14T07:59:00Z")));
        assert!(batch.keeps(&at("2026-10-14T08:00:00Z")));
        // 12:00 UTC is 09:00 in Sao Paulo (UTC-3, no DST in October 2026).
        let sp = clock(|c| {
            c.zone = jiff::tz::TimeZone::get("America/Sao_Paulo").ok();
            c.between = Some(Clock::parse_between("09:00-10:00").unwrap());
        });
        assert!(sp.keeps(&at("2026-10-14T12:30:00Z")));
        assert!(!sp.keeps(&at("2026-10-14T09:30:00Z")));
    }

    #[test]
    fn weekdays_parse_ranges_and_wrap() {
        assert_eq!(
            Clock::parse_weekdays("mon-fri").unwrap(),
            [true, true, true, true, true, false, false]
        );
        assert_eq!(
            Clock::parse_weekdays("sat,sun,mon").unwrap(),
            [true, false, false, false, false, true, true]
        );
        assert_eq!(
            Clock::parse_weekdays("fri-mon").unwrap(),
            [true, false, false, false, true, true, true]
        );
        Clock::parse_weekdays("funday").unwrap_err();
        let weekdays = clock(|c| c.weekdays = Some(Clock::parse_weekdays("mon-fri").unwrap()));
        // 2026-10-14 is a Wednesday, 2026-10-17 a Saturday.
        assert!(weekdays.keeps(&at("2026-10-14T12:00:00Z")));
        assert!(!weekdays.keeps(&at("2026-10-17T12:00:00Z")));
    }

    #[test]
    fn a_filter_describes_itself_and_an_empty_one_says_nothing() {
        assert_eq!(Filter::default().describe(), "");
        let filter = Filter {
            exclude: Facets {
                applications: names(&["pgAdmin*"]),
                ..Facets::default()
            },
            pids: vec![1, 2],
            ..Filter::default()
        };
        assert_eq!(filter.describe(), "exclude-application=pgAdmin*; pid=1,2");
    }
}
