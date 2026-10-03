//! The standalone `dagayn` binary: the commands [`dagayn_cli::run`] handles,
//! and an error naming what it does not.

use std::process::ExitCode;

use dagayn_cli::{Fallback, Outcome};

fn main() -> ExitCode {
    match dagayn_cli::run(std::env::args_os()) {
        Outcome::Exit(code) => ExitCode::from(code),
        Outcome::Fallback(Fallback::Parse(err)) => err.exit(),
        Outcome::Fallback(Fallback::Unsupported(reason)) => {
            eprintln!("ERROR: {reason}; use the Python CLI");
            ExitCode::FAILURE
        }
    }
}
