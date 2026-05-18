use clap::Parser;
use sealport_cli::{Cli, run};

fn main() {
    let cli = Cli::parse();

    match run(cli) {
        Ok(output) => {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}
