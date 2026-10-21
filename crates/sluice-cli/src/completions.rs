//! `sluice completions <shell>`: the completion script, written to stdout.

use clap_complete::Shell;
use sluice_core::Error;

use crate::output::Outcome;

#[derive(clap::Args)]
pub struct Args {
    /// The shell to write the script for
    shell: Shell,
}

/// Made from the clap definition when asked for, so no other command pays for it.
pub fn run(args: &Args) -> Result<Outcome, Error> {
    let mut script = Vec::new();
    clap_complete::generate(args.shell, &mut crate::command(), "sluice", &mut script);
    String::from_utf8(script)
        .map(Outcome::Text)
        .map_err(|e| Error::Bug(format!("the completion script is not UTF-8: {e}")))
}
