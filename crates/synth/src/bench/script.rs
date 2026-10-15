//! Statements with literal values, as pgbench custom scripts.
//!
//! pgbench substitutes `:name` variables into the text in the simple protocol, so each run of
//! a script sends a different literal. The many shapes (tables × columns × predicates × tails)
//! give the statement tracker more distinct statements than `pg_stat_statements.max` holds.
//! pgbench takes at most 128 scripts, so [`GROUP`] shapes share one script and a random pick
//! chooses among them with `\if`.

use std::{fs, path::Path};

use anyhow::Context as _;

/// Shapes per script file.
pub const GROUP: usize = 5;

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

/// A script that runs one of `statements` (at most [`GROUP`]), chosen at random.
fn script(statements: &[String]) -> String {
    if let [only] = statements {
        return format!("{HEADER}{only}\n");
    }
    let branches = statements
        .iter()
        .enumerate()
        .map(|(i, statement)| {
            let keyword = if i == 0 { "\\if" } else { "\\elif" };
            format!("{keyword} :pick = {i}\n{statement}\n")
        })
        .collect::<Vec<_>>()
        .concat();
    format!(
        "{HEADER}\\set pick random(0, {})\n{branches}\\endif\n",
        statements.len() - 1
    )
}

/// Every shape, in a fixed order.
#[must_use]
pub fn shapes() -> Vec<String> {
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
                    out.push(format!(
                        "SELECT {} FROM {} WHERE {predicate}{tail};",
                        columns.join(", "),
                        table.name
                    ));
                }
            }
        }
    }
    out
}

/// Writes the scripts into `dir` (created if missing) and returns the file names, sorted.
///
/// # Errors
///
/// Fails if the directory or a file cannot be written.
pub fn write_scripts(dir: &Path) -> anyhow::Result<Vec<String>> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut names = Vec::new();
    for (i, group) in shapes().chunks(GROUP).enumerate() {
        let name = format!("g{i:03}.sql");
        let path = dir.join(&name);
        fs::write(&path, script(group)).with_context(|| format!("writing {}", path.display()))?;
        names.push(name);
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
    fn every_shape_is_distinct_and_avoids_the_growing_table() {
        let all = shapes();
        let unique: HashSet<&String> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
        for shape in &all {
            assert!(shape.starts_with("SELECT "), "{shape}");
            assert!(shape.ends_with(';'), "{shape}");
            assert!(!shape.contains("pgbench_history"), "{shape}");
        }
    }

    #[test]
    fn a_script_picks_one_statement_with_if_branches() {
        let text = script(&["SELECT 1;".to_owned(), "SELECT 2;".to_owned()]);
        for var in [
            "\\set aid",
            "\\set bid",
            "\\set tid",
            "\\set delta",
            "\\set pick random(0, 1)",
        ] {
            assert!(text.contains(var), "{var}");
        }
        assert!(text.contains("\\if :pick = 0\nSELECT 1;\n\\elif :pick = 1\nSELECT 2;\n\\endif\n"));
    }

    #[test]
    fn scripts_stay_within_the_pgbench_limit() {
        let dir = env::temp_dir().join(format!("synth-script-{}", process::id()));
        let names = write_scripts(&dir).unwrap();
        assert!(names.len() <= 128, "{}", names.len());
        assert_eq!(names.len(), shapes().len().div_ceil(GROUP));
        assert!(names.windows(2).all(|w| w[0] < w[1]));
        let first = fs::read_to_string(dir.join(&names[0])).unwrap();
        assert!(first.contains("SELECT"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
