use clap::Parser;

/// A close look at your Postgres logs and query stats.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli;

fn main() {
    Cli::parse();
    println!("pgloupe {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }
}
