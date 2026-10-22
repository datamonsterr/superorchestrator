#![cfg_attr(test, allow(clippy::unwrap_used))]

use clap::Parser;

/// Command line interface for superorchestrator.
#[derive(Parser, Debug)]
#[command(
    name = "superorchestrator",
    version,
    about = "Delegate coding work across harnesses and accounts"
)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }
}
