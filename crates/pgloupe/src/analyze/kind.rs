//! The class a statement belongs to (SELECT, INSERT, DDL, ...), for the query-type table.

use serde::{Deserialize, Serialize};

/// What a statement does, as far as a report reader cares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Select,
    Insert,
    Update,
    Delete,
    Copy,
    /// `CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `GRANT` and friends.
    Ddl,
    /// `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT` and friends.
    Tcl,
    /// `VACUUM`, `SET`, `EXPLAIN`, `CALL`, `LOCK` and other utility statements.
    Utility,
    /// Unparsable text, `MERGE`, and the log's overflow bucket.
    #[default]
    Other,
}

impl QueryKind {
    /// Every kind, in the order reports list them.
    pub const ALL: [QueryKind; 9] = [
        QueryKind::Select,
        QueryKind::Insert,
        QueryKind::Update,
        QueryKind::Delete,
        QueryKind::Copy,
        QueryKind::Ddl,
        QueryKind::Tcl,
        QueryKind::Utility,
        QueryKind::Other,
    ];

    /// The name used in reports.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            QueryKind::Select => "select",
            QueryKind::Insert => "insert",
            QueryKind::Update => "update",
            QueryKind::Delete => "delete",
            QueryKind::Copy => "copy",
            QueryKind::Ddl => "ddl",
            QueryKind::Tcl => "tcl",
            QueryKind::Utility => "utility",
            QueryKind::Other => "other",
        }
    }
}
