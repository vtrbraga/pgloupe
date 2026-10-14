use std::{
    env, fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

/// Fresh empty directory under the system temp dir.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(unique(name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Name unique across processes and calls.
pub fn unique(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("synth-test-{name}-{}-{nanos}", process::id())
}
