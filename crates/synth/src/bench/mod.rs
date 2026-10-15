//! Large-dataset validation: a pgbench-driven run that fills a log directory to a target size.

pub mod pgbench;
pub mod script;
pub mod size;

/// Server settings for a bench run, on top of [`crate::container::SETTINGS`].
///
/// The file name carries a timestamp so size-based rotation starts a new file instead of
/// appending to the old one; csvlog and jsonlog files take the same name with their own suffix.
pub const SETTINGS: &[(&str, &str)] = &[
    ("log_filename", "postgresql-%Y%m%d-%H%M%S.log"),
    ("log_rotation_size", "256MB"),
    ("max_wal_size", "256MB"),
    ("checkpoint_timeout", "30s"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_file_names_keep_the_log_suffix() {
        let (_, name) = SETTINGS.iter().find(|(n, _)| *n == "log_filename").unwrap();
        assert_eq!(name.rsplit('.').next(), Some("log"));
        assert!(
            name.contains("%S"),
            "rotation within a second would collide"
        );
    }
}
