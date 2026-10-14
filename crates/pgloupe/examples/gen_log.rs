//! Writes a synthetic jsonlog for benchmarks: many hours, many distinct statements.
//!
//! `cargo run --release --example gen_log -- OUT RECORDS HOURS FINGERPRINTS [SEED]`
//!
//! Statements look like `SELECT * FROM tenant_07.events_pabcd WHERE id = 99`, so every
//! (tenant, partition) pair is its own fingerprint, as in a multi-tenant partitioned database.
//! Partition names end in letters: `libpg_query`'s fingerprint ignores trailing digits in table
//! names, which would merge `events_p0001` and `events_p0002`.

use std::{
    fs::File,
    io::{BufWriter, Write},
    process::ExitCode,
};

/// Deterministic generator, so a benchmark input is reproducible.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = (|| {
        let out = args.first()?.clone();
        let records: u64 = args.get(1)?.parse().ok()?;
        let hours: u64 = args.get(2)?.parse().ok()?;
        let fingerprints: u64 = args.get(3)?.parse().ok()?;
        let seed: u64 = args.get(4).map_or(Some(1), |s| s.parse().ok())?;
        Some((out, records, hours.max(1), fingerprints.max(1), seed))
    })();
    let Some((out, records, hours, fingerprints, seed)) = parsed else {
        eprintln!("usage: gen_log OUT RECORDS HOURS FINGERPRINTS [SEED]");
        return ExitCode::from(2);
    };
    match write(&out, records, hours, fingerprints, seed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Four lowercase letters standing for `n` (base 26).
fn letters(mut n: u64) -> String {
    let mut name = [b'a'; 4];
    for slot in name.iter_mut().rev() {
        *slot = b'a' + u8::try_from(n % 26).unwrap_or(0);
        n /= 26;
    }
    String::from_utf8_lossy(&name).into_owned()
}

fn write(out: &str, records: u64, hours: u64, fingerprints: u64, seed: u64) -> std::io::Result<()> {
    let mut file = BufWriter::new(File::create(out)?);
    let mut rng = Lcg(seed);
    // 2026-10-01 00:00:00 UTC, so the log starts on a day boundary.
    let start: u64 = 1_790_812_800;
    for i in 0..records {
        let second = start + i * hours * 3_600 / records;
        let (day, rest) = ((second - start) / 86_400, (second - start) % 86_400);
        let (hour, minute, sec) = (rest / 3_600, rest % 3_600 / 60, rest % 60);
        let fingerprint = rng.next() % fingerprints;
        let (tenant, partition) = (fingerprint % 50, fingerprint / 50);
        let centi = 20 + rng.next() % 50_000;
        // Day 0 = 2026-10-01; the generator stays inside one month.
        let day = 1 + day % 28;
        let millis = rng.next() % 1_000;
        let pid = 1_000 + rng.next() % 200;
        let session = rng.next() % 100;
        let id = rng.next() % 1_000_000;
        let part = letters(partition);
        let (units, hundredths) = (centi / 100, centi % 100);
        writeln!(
            file,
            r#"{{"timestamp":"2026-10-{day:02} {hour:02}:{minute:02}:{sec:02}.{millis:03} UTC","user":"app","dbname":"main","pid":{pid},"session_id":"a.{session}","line_num":1,"error_severity":"LOG","message":"duration: {units}.{hundredths:02} ms  statement: SELECT * FROM tenant_{tenant:02}.events_p{part} WHERE id = {id}","backend_type":"client backend","query_id":0}}"#,
        )?;
    }
    file.flush()
}
