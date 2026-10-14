use std::{
    io::{self, BufWriter, Write},
    process::ExitCode,
};

use clap::Parser as _;
use pgloupe::cli::{Cli, Exit, run};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut out = BufWriter::new(io::stdout().lock());
    let result = run(&cli, &mut out, &mut io::stderr().lock());
    let flushed = out.flush();
    match (result, flushed) {
        (Ok(Exit::Ok), Ok(())) => ExitCode::SUCCESS,
        (Ok(Exit::Failed), _) => ExitCode::FAILURE,
        (Err(err), _) if is_broken_pipe(&err) => ExitCode::SUCCESS,
        (Err(err), _) => {
            let _ = writeln!(io::stderr(), "error: {err:#}");
            ExitCode::FAILURE
        }
        (Ok(Exit::Ok), Err(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        (Ok(Exit::Ok), Err(err)) => {
            let _ = writeln!(io::stderr(), "error: writing output: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `pgloupe ... | head` closes the pipe early; that is not an error.
fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let kind = cause
            .downcast_ref::<io::Error>()
            .map(io::Error::kind)
            .or_else(|| {
                cause
                    .downcast_ref::<serde_json::Error>()
                    .and_then(serde_json::Error::io_error_kind)
            });
        kind == Some(io::ErrorKind::BrokenPipe)
    })
}
