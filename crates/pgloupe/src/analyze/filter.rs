//! Choosing which records of a log take part in an analysis.

use regex::Regex;

use crate::log::LogRecord;

/// Values a record must (include) or must not (exclude) carry, per attribute.
#[derive(Debug, Clone, Default)]
pub struct Facets {
    pub databases: Vec<String>,
    pub users: Vec<String>,
    pub applications: Vec<String>,
    pub hosts: Vec<String>,
}

impl Facets {
    fn is_empty(&self) -> bool {
        self.databases.is_empty()
            && self.users.is_empty()
            && self.applications.is_empty()
            && self.hosts.is_empty()
    }

    fn pairs<'a>(&'a self, record: &'a LogRecord) -> [(&'a [String], Option<&'a str>); 4] {
        [
            (&self.databases, record.database.as_deref()),
            (&self.users, record.user.as_deref()),
            (&self.applications, record.application.as_deref()),
            (&self.hosts, record.host.as_deref()),
        ]
    }
}

/// Which records are kept: every include list that is set must contain the record's value, no
/// exclude list may, and the statement patterns must agree.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub include: Facets,
    pub exclude: Facets,
    /// Keep only records whose message or statement matches.
    pub include_query: Option<Regex>,
    /// Drop records whose message or statement matches.
    pub exclude_query: Option<Regex>,
}

impl Filter {
    /// Whether the filter keeps everything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.include.is_empty()
            && self.exclude.is_empty()
            && self.include_query.is_none()
            && self.exclude_query.is_none()
    }

    /// Whether `record` takes part in the analysis. A record without an attribute fails an include
    /// list for it and passes an exclude list.
    #[must_use]
    pub fn keeps(&self, record: &LogRecord) -> bool {
        for (wanted, actual) in self.include.pairs(record) {
            if !wanted.is_empty() && !actual.is_some_and(|value| wanted.iter().any(|w| w == value))
            {
                return false;
            }
        }
        for (unwanted, actual) in self.exclude.pairs(record) {
            if actual.is_some_and(|value| unwanted.iter().any(|u| u == value)) {
                return false;
            }
        }
        let matches = |pattern: &Regex| {
            pattern.is_match(&record.message)
                || record
                    .statement
                    .as_deref()
                    .is_some_and(|statement| pattern.is_match(statement))
        };
        if let Some(pattern) = &self.include_query
            && !matches(pattern)
        {
            return false;
        }
        if let Some(pattern) = &self.exclude_query
            && matches(pattern)
        {
            return false;
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

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
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
}
