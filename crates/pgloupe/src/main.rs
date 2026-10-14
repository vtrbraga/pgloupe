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
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
        (Ok(Exit::Ok), Err(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        (Ok(Exit::Ok), Err(err)) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `pgloupe ... | head` closes the pipe early; that is not an error.
fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|io| io.kind() == io::ErrorKind::BrokenPipe)
    })
}
