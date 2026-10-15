//! Providers whose vendor hands out log entries rather than files write them as hourly files of
//! lines. This is the one place that does the grouping, the de-duplication and the resume cursor.

use std::collections::BTreeSet;

use jiff::{Timestamp, tz::TimeZone};
use serde::{Deserialize, Serialize};

use super::{FetchError, LogSink, ProviderState};

/// One log entry as the vendor reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// When the vendor says it was logged.
    pub ts: Timestamp,
    /// A stable id, to tell entries with the same timestamp apart (empty if the vendor has none).
    pub id: String,
    /// The log text, without a trailing newline (may span several lines).
    pub text: String,
}

/// Where the last run stopped: everything at or before `ts` was seen, with `ids` the entries at
/// exactly `ts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// The newest timestamp written.
    pub ts: Option<Timestamp>,
    /// Entries written at exactly `ts`.
    pub ids: BTreeSet<String>,
}

impl Cursor {
    /// The cursor saved under `key`, or an empty one.
    #[must_use]
    pub fn load(state: &ProviderState, key: &str) -> Cursor {
        state
            .get(key)
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default()
    }

    /// Saves the cursor under `key`.
    pub fn store(&self, state: &mut ProviderState, key: &str) {
        if let Ok(text) = serde_json::to_string(self) {
            state.insert(key.to_owned(), text);
        }
    }

    /// Whether `entry` has not been written yet.
    #[must_use]
    pub fn is_new(&self, entry: &Entry) -> bool {
        match self.ts {
            None => true,
            Some(ts) => entry.ts > ts || (entry.ts == ts && !self.ids.contains(&entry.id)),
        }
    }

    fn advance(&mut self, entry: &Entry) {
        match self.ts {
            Some(ts) if entry.ts < ts => {}
            Some(ts) if entry.ts == ts => {
                self.ids.insert(entry.id.clone());
            }
            _ => {
                self.ts = Some(entry.ts);
                self.ids.clear();
                self.ids.insert(entry.id.clone());
            }
        }
    }
}

/// `<prefix>-YYYYMMDD-HH.log`: the hour (UTC) the entry belongs to.
#[must_use]
pub fn hourly_name(prefix: &str, ts: Timestamp) -> String {
    let at = ts.to_zoned(TimeZone::UTC);
    format!(
        "{}-{:04}{:02}{:02}-{:02}.log",
        super::aws::safe_name(prefix),
        at.year(),
        at.month(),
        at.day(),
        at.hour()
    )
}

/// Writes the entries newer than `cursor` (oldest first) into hourly files and moves the cursor.
/// Returns the files written to and the bytes added.
///
/// # Errors
///
/// Fails if a file cannot be written.
pub fn write(
    sink: &mut dyn LogSink,
    prefix: &str,
    entries: &[Entry],
    cursor: &mut Cursor,
) -> Result<(Vec<String>, u64), FetchError> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));
    let mut files: Vec<String> = Vec::new();
    let mut bytes = 0;
    for entry in sorted {
        if !cursor.is_new(entry) {
            continue;
        }
        let name = hourly_name(prefix, entry.ts);
        let mut line = entry.text.clone();
        if !line.ends_with('\n') {
            line.push('\n');
        }
        sink.append(&name, line.as_bytes())?;
        bytes += line.len() as u64;
        if !files.contains(&name) {
            files.push(name);
        }
        cursor.advance(entry);
    }
    Ok((files, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Mem(std::collections::BTreeMap<String, String>);

    impl LogSink for Mem {
        fn stored(&self, name: &str) -> u64 {
            self.0.get(name).map_or(0, |s| s.len() as u64)
        }
        fn reset(&mut self, name: &str) -> Result<(), FetchError> {
            self.0.remove(name);
            Ok(())
        }
        fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), FetchError> {
            self.0
                .entry(name.to_owned())
                .or_default()
                .push_str(&String::from_utf8_lossy(bytes));
            Ok(())
        }
        fn checkpoint(&mut self, _state: &ProviderState) -> Result<(), FetchError> {
            Ok(())
        }
    }

    fn entry(ts: &str, id: &str, text: &str) -> Entry {
        Entry {
            ts: ts.parse().unwrap(),
            id: id.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn entries_land_in_hourly_files_in_time_order_with_newlines() {
        let mut sink = Mem::default();
        let mut cursor = Cursor::default();
        let entries = [
            entry("2026-10-14T11:00:05Z", "b", "second hour"),
            entry("2026-10-14T10:59:59Z", "a", "first hour\ncontinued"),
        ];
        let (files, bytes) = write(&mut sink, "gcp/prod", &entries, &mut cursor).unwrap();
        assert_eq!(
            files,
            ["gcp_prod-20261014-10.log", "gcp_prod-20261014-11.log"]
        );
        assert_eq!(
            sink.0["gcp_prod-20261014-10.log"],
            "first hour\ncontinued\n"
        );
        assert_eq!(bytes, 21 + 12);
    }

    #[test]
    fn a_second_batch_skips_what_was_written_even_at_the_same_timestamp() {
        let mut sink = Mem::default();
        let mut cursor = Cursor::default();
        let first = [entry("2026-10-14T10:00:00Z", "1", "one")];
        write(&mut sink, "p", &first, &mut cursor).unwrap();
        let again = [
            entry("2026-10-14T10:00:00Z", "1", "one"),
            entry("2026-10-14T10:00:00Z", "2", "two, same instant"),
            entry("2026-10-14T10:00:01Z", "3", "three"),
        ];
        write(&mut sink, "p", &again, &mut cursor).unwrap();
        assert_eq!(
            sink.0["p-20261014-10.log"],
            "one\ntwo, same instant\nthree\n"
        );
    }

    #[test]
    fn the_cursor_survives_a_round_trip_through_the_state() {
        let mut cursor = Cursor::default();
        cursor.advance(&entry("2026-10-14T10:00:00Z", "x", "t"));
        let mut state = ProviderState::new();
        cursor.store(&mut state, "cursor");
        assert_eq!(Cursor::load(&state, "cursor"), cursor);
        assert_eq!(Cursor::load(&state, "other"), Cursor::default());
    }
}
