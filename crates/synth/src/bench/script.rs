//! Statements with literal values, as pgbench custom scripts.
//!
//! pgbench substitutes `:name` variables into the text in the simple protocol, so each run of
//! a script sends a different literal. The many shapes (tables × columns × predicates × tails)
//! give the statement tracker more distinct statements than `pg_stat_statements.max` holds.

use std::{fs, path::Path};

use anyhow::Context as _;

/// One statement shape and the file name it is written to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// File name, e.g. `s0007.sql`.
    pub name: String,
    /// pgbench script text.
    pub text: String,
}

struct Table {
    name: &'static str,
    key: &'static str,
    var: &'static str,
    balance: &'static str,
    columns: &'static [&'static str],
}

const TABLES: &[Table] = &[
    Table {
        name: "pgbench_accounts",
        key: "aid",
        var: "aid",
        balance: "abalance",
        columns: &["aid", "bid", "abalance", "filler"],
    },
    Table {
        name: "pgbench_tellers",
        key: "tid",
        var: "tid",
        balance: "tbalance",
        columns: &["tid", "bid", "tbalance"],
    },
    Table {
        name: "pgbench_branches",
        key: "bid",
        var: "bid",
        balance: "bbalance",
        columns: &["bid", "bbalance"],
    },
];

const HEADER: &str = "\\set aid random(1, 100000 * :scale)\n\
                      \\set bid random(1, :scale)\n\
                      \\set tid random(1, 10 * :scale)\n\
                      \\set delta random(-5000, 5000)\n";

/// Every shape, in a fixed order.
#[must_use]
pub fn shapes() -> Vec<Shape> {
    let mut out = Vec::new();
    for table in TABLES {
        let (k, v, bal) = (table.key, table.var, table.balance);
        let predicates = [
            format!("{k} = :{v}"),
            format!("{k} BETWEEN :{v} AND :{v} + 10"),
            format!("{k} >= :{v} AND {k} < :{v} + 100"),
            format!("{k} IN (:{v}, :{v} + 1)"),
            format!("{bal} > :delta AND {k} = :{v}"),
        ];
        let tails = [
            String::new(),
            format!(" ORDER BY {k}"),
            format!(" ORDER BY {k} LIMIT 10"),
            format!(" ORDER BY {k} DESC LIMIT 5"),
            " LIMIT 1".to_owned(),
        ];
        for mask in 1_u32..(1 << table.columns.len()) {
            let columns: Vec<&str> = table
                .columns
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, c)| *c)
                .collect();
            for predicate in &predicates {
                for tail in &tails {
                    let text = format!(
                        "{HEADER}SELECT {} FROM {} WHERE {predicate}{tail};\n",
                        columns.join(", "),
                        table.name
                    );
                    out.push(Shape {
                        name: format!("s{:04}.sql", out.len()),
                        text,
                    });
                }
            }
        }
    }
    out
}

/// Writes every shape into `dir` (created if missing) and returns the file names, sorted.
///
/// # Errors
///
/// Fails if the directory or a file cannot be written.
pub fn write_scripts(dir: &Path) -> anyhow::Result<Vec<String>> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut names = Vec::new();
    for shape in shapes() {
        let path = dir.join(&shape.name);
        fs::write(&path, &shape.text).with_context(|| format!("writing {}", path.display()))?;
        names.push(shape.name);
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, env, process};

    use super::*;

    #[test]
    fn there_are_more_shapes_than_the_tracker_holds() {
        assert!(shapes().len() >= 600, "{}", shapes().len());
    }

    #[test]
    fn every_shape_is_distinct_and_defines_its_variables() {
        let all = shapes();
        let texts: HashSet<&str> = all.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts.len(), all.len());
        for shape in &all {
            for var in ["\\set aid", "\\set bid", "\\set tid", "\\set delta"] {
                assert!(shape.text.contains(var), "{}", shape.name);
            }
            assert!(shape.text.ends_with(";\n"), "{}", shape.name);
            assert!(!shape.text.contains("pgbench_history"), "{}", shape.name);
        }
    }

    #[test]
    fn scripts_are_written_one_file_per_shape() {
        let dir = env::temp_dir().join(format!("synth-script-{}", process::id()));
        let names = write_scripts(&dir).unwrap();
        assert_eq!(names.len(), shapes().len());
        assert!(names.windows(2).all(|w| w[0] < w[1]));
        let first = fs::read_to_string(dir.join(&names[0])).unwrap();
        assert!(first.contains("SELECT"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
