//! `cargo xtask bench`: writes synthetic stderr logs and times `pgloupe logs analyze` on them.
//!
//! `gen CASE` fills `data/bench/CASE/` (never committed); `run CASE` analyzes it at each thread
//! count and fails when the report differs between thread counts.

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write as _,
    fs::{self, File},
    hash::Hasher as _,
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand, ValueEnum};

/// Bytes written per case unless `--size` says otherwise.
const DEFAULT_SIZE: u64 = 2 << 30;
/// The first record: 2026-10-15 06:00:00 UTC, in milliseconds since the epoch.
const START_MS: u64 = 1_792_044_000_000;
/// Backend sessions the records are spread over.
const SESSIONS: usize = 64;

#[derive(Parser)]
#[command(name = "xtask bench", about = "Benchmark input and timings")]
struct Cli {
    #[command(subcommand)]
    command: Step,
}

#[derive(Subcommand)]
enum Step {
    /// Write the input for a case into data/bench/CASE.
    Gen {
        case: Case,
        /// Total size, such as 500M or 2G.
        #[arg(long, value_parser = parse_size)]
        size: Option<u64>,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Time `pgloupe logs analyze` on a case at each thread count.
    Run {
        case: Case,
        #[arg(long, value_delimiter = ',', default_values_t = [1, 4, 8, 16])]
        threads: Vec<usize>,
        /// Runs per thread count; the fastest is reported.
        #[arg(long, default_value_t = 1)]
        runs: usize,
        /// The binary to time (default: target/release/pgloupe).
        #[arg(long)]
        bin: Option<PathBuf>,
    },
}

/// A shape of input: how many files, and how many distinct statements.
#[derive(Clone, Copy, ValueEnum)]
enum Case {
    /// One file, 600 statement shapes.
    Single,
    /// Eight files, 600 statement shapes.
    Files,
    /// Eight files, 100,000 statement shapes (the fingerprint cache misses).
    Wide,
}

impl Case {
    fn name(self) -> &'static str {
        match self {
            Case::Single => "single",
            Case::Files => "files",
            Case::Wide => "wide",
        }
    }

    fn files(self) -> u64 {
        match self {
            Case::Single => 1,
            Case::Files | Case::Wide => 8,
        }
    }

    fn shapes(self) -> u64 {
        match self {
            Case::Single | Case::Files => 600,
            Case::Wide => 100_000,
        }
    }
}

#[expect(clippy::cast_precision_loss, reason = "a size shown to three digits")]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

fn parse_size(text: &str) -> Result<u64, String> {
    let (digits, unit) = text.split_at(text.len().saturating_sub(1));
    let factor = match unit {
        "K" | "k" => 1 << 10,
        "M" | "m" => 1 << 20,
        "G" | "g" => 1 << 30,
        _ => return text.parse().map_err(|e| format!("{e}")),
    };
    let count: u64 = digits.parse().map_err(|e| format!("{e}"))?;
    Ok(count * factor)
}

/// Runs the subcommand named by `args` (the words after `bench`).
///
/// # Errors
///
/// Fails when a file cannot be written, the binary cannot run, or the report depends on the
/// number of threads.
pub fn run(root: &Path, args: &[String]) -> anyhow::Result<()> {
    let root = &root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let cli =
        Cli::parse_from(std::iter::once("xtask bench".to_owned()).chain(args.iter().cloned()));
    match cli.command {
        Step::Gen { case, size, seed } => generate(
            &case_dir(root, case),
            case,
            size.unwrap_or(DEFAULT_SIZE),
            seed,
        ),
        Step::Run {
            case,
            threads,
            runs,
            bin,
        } => {
            let bin = bin.unwrap_or_else(|| root.join("target/release/pgloupe"));
            time(&case_dir(root, case), &bin, &threads, runs.max(1))
        }
    }
}

fn case_dir(root: &Path, case: Case) -> PathBuf {
    root.join("data/bench").join(case.name())
}

/// Deterministic generator, so an input is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Four lowercase letters standing for `n`. Names must end in letters: the fingerprint ignores
/// trailing digits in table names, which would merge distinct shapes.
fn letters(mut n: u64) -> String {
    let mut name = [b'a'; 4];
    for slot in name.iter_mut().rev() {
        *slot = b'a' + u8::try_from(n % 26).unwrap_or(0);
        n /= 26;
    }
    String::from_utf8_lossy(&name).into_owned()
}

/// Days since 1970-01-01 to (year, month, day).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

fn timestamp(ms: u64) -> String {
    let (year, month, day) = civil(ms / 86_400_000);
    let rest = ms % 86_400_000;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03} UTC",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

struct Session {
    pid: u32,
    who: String,
}

/// One statement of shape `shape`, with fresh literals.
fn statement(rng: &mut Rng, shape: u64, extended: bool) -> String {
    let table = letters(shape / 6);
    let other = letters(shape / 6 + 1);
    let mut n = || {
        if extended {
            "$1".to_owned()
        } else {
            rng.below(1_000_000).to_string()
        }
    };
    match shape % 6 {
        0 => format!("SELECT c1, c2 FROM t_{table} WHERE id = {}", n()),
        1 => format!(
            "SELECT a.x, b.y FROM t_{table} a JOIN t_{other} b ON b.id = a.id WHERE a.k = {} AND b.z > {}",
            n(),
            n()
        ),
        2 => format!(
            "UPDATE t_{table} SET v = {}, ts = now() WHERE id = {}",
            n(),
            n()
        ),
        3 => format!(
            "INSERT INTO t_{table} (id, v, note) VALUES ({}, {}, 'note')",
            n(),
            n()
        ),
        4 => format!(
            "SELECT count(*), sum(v) FROM t_{table} WHERE k IN ({}, {}, {}) GROUP BY g",
            n(),
            n(),
            n()
        ),
        _ => format!(
            "DELETE FROM t_{table} WHERE ts < now() - interval '30 days' AND id = {}",
            n()
        ),
    }
}

/// A duration in milliseconds with a long tail.
fn duration(rng: &mut Rng) -> String {
    let micros = if rng.below(100) == 0 {
        rng.below(2_000_000)
    } else {
        let ceiling = rng.below(20_000) + 1;
        20 + rng.below(ceiling)
    };
    format!("{}.{:03}", micros / 1_000, micros % 1_000)
}

fn generate(dir: &Path, case: Case, size: u64, seed: u64) -> anyhow::Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    fs::create_dir_all(dir)?;
    let mut rng = Rng(seed.max(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let sessions: Vec<Session> = (0..SESSIONS)
        .map(|i| Session {
            pid: 1_000 + u32::try_from(i).unwrap_or(0) * 7,
            who: format!("user{}@db{}/app{}", i % 5, i % 3, i % 11),
        })
        .collect();
    let per_file = size / case.files();
    let mut clock = START_MS;
    let mut line = String::new();
    for index in 0..case.files() {
        let path = dir.join(format!("postgresql-{index:02}.log"));
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        let mut written = 0u64;
        while written < per_file {
            clock += 1 + rng.below(8);
            line.clear();
            let session = &sessions[usize::try_from(rng.below(SESSIONS as u64))?];
            record(&mut line, &mut rng, session, clock, case.shapes())?;
            out.write_all(line.as_bytes())?;
            written += line.len() as u64;
        }
        out.flush()?;
    }
    report(dir)
}

/// Appends one event: a statement, a connection, an error, ... as the server would log it.
fn record(
    out: &mut String,
    rng: &mut Rng,
    session: &Session,
    clock: u64,
    shapes: u64,
) -> anyhow::Result<()> {
    let at = timestamp(clock);
    let pid = session.pid;
    let who = &session.who;
    let shape = rng.below(shapes);
    let query_id = i64::try_from(shape.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 2)?;
    let head = format!("{at} [{pid}] {who} {query_id}");
    let kind = rng.below(1_000);
    if kind < 720 {
        let (ms, sql) = (duration(rng), statement(rng, shape, false));
        writeln!(out, "{head} LOG:  duration: {ms} ms  statement: {sql}")?;
    } else if kind < 860 {
        let (ms, sql) = (duration(rng), statement(rng, shape, true));
        let (a, b) = (rng.below(1_000_000), rng.below(1_000_000));
        writeln!(
            out,
            "{head} LOG:  duration: {ms} ms  execute s{}: {sql}",
            shape % 40
        )?;
        writeln!(
            out,
            "{head} DETAIL:  parameters: $1 = '{a}', $2 = '{b}', $3 = '{a}'"
        )?;
    } else if kind < 900 {
        writeln!(
            out,
            "{at} [{pid}] [unknown]@[unknown]/[unknown] 0 LOG:  connection received: host=10.0.0.{} port={}",
            rng.below(250),
            30_000 + rng.below(30_000)
        )?;
        writeln!(
            out,
            "{head} LOG:  connection authorized: user={} database=db application_name=app",
            who.split('@').next().unwrap_or("u")
        )?;
    } else if kind < 940 {
        let ms = duration(rng);
        let table = letters(shape / 6);
        writeln!(out, "{head} LOG:  duration: {ms} ms  statement: SELECT a,")?;
        writeln!(
            out,
            "\tb,\n\tc\n\tFROM t_{table}\n\tWHERE id = {}",
            rng.below(1_000_000)
        )?;
    } else if kind < 955 {
        let sql = statement(rng, shape, false);
        writeln!(
            out,
            "{head} ERROR:  relation \"t_{}\" does not exist at character 15",
            letters(shape / 6)
        )?;
        writeln!(out, "{head} STATEMENT:  {sql}")?;
    } else if kind < 960 {
        writeln!(out, "{at} [{pid}] LOG:  checkpoint starting: time")?;
        writeln!(
            out,
            "{at} [{pid}] LOG:  checkpoint complete: wrote {} buffers (1.2%); 0 WAL file(s) added, 0 removed, 1 recycled; write=29.9 s, sync=0.1 s, total=30.1 s",
            100 + rng.below(50_000)
        )?;
    } else if kind < 975 {
        let sql = statement(rng, shape, false);
        writeln!(
            out,
            "{head} LOG:  temporary file: path \"base/pgsql_tmp/pgsql_tmp{pid}.0\", size {}",
            1_000 + rng.below(50_000_000)
        )?;
        writeln!(out, "{head} STATEMENT:  {sql}")?;
    } else {
        writeln!(
            out,
            "{at} [{pid}] LOG:  automatic vacuum of table \"db.public.t_{}\": index scans: 1, pages: 0 removed, 120 remain, tuples: 40 removed, 900 remain",
            letters(shape / 6)
        )?;
    }
    Ok(())
}

#[expect(clippy::print_stdout, reason = "the report is the output")]
fn report(dir: &Path) -> anyhow::Result<()> {
    let (files, bytes) = inputs(dir)?;
    println!(
        "{}: {} files, {:.0} MiB",
        dir.display(),
        files.len(),
        mib(bytes)
    );
    Ok(())
}

/// The case's files in name order, and their total size.
fn inputs(dir: &Path) -> anyhow::Result<(Vec<PathBuf>, u64)> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("{} is missing; run `cargo xtask bench gen`", dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.sort();
    let mut bytes = 0;
    for file in &files {
        bytes += fs::metadata(file)?.len();
    }
    Ok((files, bytes))
}

/// One timed run: seconds, peak MiB (where `/usr/bin/time` exists) and a digest of the report.
fn once(bin: &Path, files: &[PathBuf], threads: usize) -> anyhow::Result<(f64, Option<f64>, u64)> {
    let timer = Path::new("/usr/bin/time");
    let mut command = if timer.exists() {
        let mut command = Command::new(timer);
        command.args(["-f", "%M"]).arg(bin);
        command
    } else {
        Command::new(bin)
    };
    command.args(["logs", "analyze"]).args(files).args([
        "--threads",
        &threads.to_string(),
        "--output",
        "json",
    ]);
    let started = Instant::now();
    let output = command
        .output()
        .with_context(|| format!("running {}", bin.display()))?;
    let seconds = started.elapsed().as_secs_f64();
    if !output.status.success() {
        bail!(
            "pgloupe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let peak = timer
        .exists()
        .then(|| {
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()?
                .trim()
                .parse::<f64>()
                .ok()
        })
        .flatten()
        .map(|kib| kib / 1_024.0);
    let mut hasher = DefaultHasher::new();
    hasher.write(&output.stdout);
    Ok((seconds, peak, hasher.finish()))
}

#[expect(clippy::print_stdout, reason = "the report is the output")]
fn time(dir: &Path, bin: &Path, threads: &[usize], runs: usize) -> anyhow::Result<()> {
    if !bin.exists() {
        bail!("{} is missing; run `cargo build --release`", bin.display());
    }
    let (files, bytes) = inputs(dir)?;
    let megabytes = mib(bytes);
    println!(
        "{}: {} files, {megabytes:.0} MiB",
        dir.display(),
        files.len()
    );
    println!("threads  seconds     MiB/s  speedup  peak MiB  report");
    let mut first: Option<(f64, u64)> = None;
    let mut differing = Vec::new();
    for &count in threads {
        let mut best: Option<(f64, Option<f64>, u64)> = None;
        for _ in 0..runs {
            let run = once(bin, &files, count)?;
            if best.is_none_or(|b| run.0 < b.0) {
                best = Some(run);
            }
        }
        let Some((seconds, peak, digest)) = best else {
            continue;
        };
        let (base, expected) = *first.get_or_insert((seconds, digest));
        if digest != expected {
            differing.push(count);
        }
        let peak = peak.map_or_else(|| "-".to_owned(), |p| format!("{p:.0}"));
        println!(
            "{count:>7}  {seconds:>7.2}  {:>8.0}  {:>6.2}x  {peak:>8}  {digest:016x}",
            megabytes / seconds,
            base / seconds
        );
    }
    if !differing.is_empty() {
        bail!("the report differs from the first run at threads {differing:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_take_a_unit() {
        assert_eq!(parse_size("500M"), Ok(500 << 20));
        assert_eq!(parse_size("2G"), Ok(2 << 30));
        assert_eq!(parse_size("1234"), Ok(1_234));
        parse_size("2X").unwrap_err();
    }

    #[test]
    fn the_clock_starts_at_the_stated_instant() {
        assert_eq!(timestamp(START_MS), "2026-10-15 06:00:00.000 UTC");
        assert_eq!(
            timestamp(START_MS + 86_400_000 + 61_001),
            "2026-10-16 06:01:01.001 UTC"
        );
    }

    #[test]
    fn shapes_get_distinct_table_names() {
        let names: std::collections::HashSet<_> = (0..5_000).map(letters).collect();
        assert_eq!(names.len(), 5_000);
        assert!(letters(12_345).bytes().all(|b| b.is_ascii_lowercase()));
    }

    #[test]
    fn the_same_seed_writes_the_same_bytes() {
        let dir = std::env::temp_dir().join(format!("xtask-bench-{}", std::process::id()));
        let read = |seed| {
            generate(&dir, Case::Files, 200_000, seed).unwrap();
            let mut all = Vec::new();
            for file in inputs(&dir).unwrap().0 {
                all.extend(fs::read(file).unwrap());
            }
            all
        };
        let (a, b, c) = (read(1), read(1), read(2));
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.len() >= 200_000);
    }
}
