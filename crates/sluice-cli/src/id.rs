//! `sluice id`: print the endpoint id of this machine, or replace the identity.

use std::io::{self, IsTerminal, Write};

use sluice_core::{Error, Home, Identity};

#[derive(clap::Args)]
pub struct Args {
    /// Print {"endpoint_id":"..."} and a newline instead of the bare id
    #[arg(long)]
    json: bool,
    /// Replace the identity with a new one, after asking
    #[arg(long)]
    rotate: bool,
    /// Do not ask before rotating
    #[arg(long, requires = "rotate")]
    yes: bool,
}

pub fn run(args: &Args) -> Result<(), Error> {
    // Before the home is opened, so a refusal leaves no folder or lock file behind.
    if args.rotate {
        confirm_rotation(args.yes)?;
    }
    let home = Home::open(&Home::locate()?)?;
    let identity = if args.rotate {
        home.rotate_identity()?
    } else {
        home.identity()?
    };
    print_id(&identity, args.json)
}

fn print_id(identity: &Identity, json: bool) -> Result<(), Error> {
    let id = identity.endpoint_id_hex();
    let mut stdout = io::stdout().lock();
    // Not `println!`, which panics on a closed pipe such as `sluice id | head -0`.
    let written = if json {
        writeln!(stdout, "{{\"endpoint_id\":\"{id}\"}}")
    } else {
        writeln!(stdout, "{id}")
    };
    written.map_err(|e| Error::Storage(format!("cannot write to standard output: {e}")))
}

/// Rotating breaks every pairing and every unexpired ticket, so it is asked about unless `yes`.
/// With no terminal to ask on it refuses, because a script that rotates by accident has no one to
/// notice.
fn confirm_rotation(yes: bool) -> Result<(), Error> {
    if yes {
        return Ok(());
    }
    if !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        return Err(Error::Policy(
            "--rotate asks before it replaces the identity, and there is no terminal to ask on. \
             Pass --yes to rotate without asking"
                .to_string(),
        ));
    }
    // On stderr, so that stdout holds the id and nothing else.
    let mut stderr = io::stderr().lock();
    write!(
        stderr,
        "Rotating replaces this identity. Every pairing and every unexpired ticket that names the \
         current endpoint id stops working.\nRotate? [y/N] "
    )
    .and_then(|()| stderr.flush())
    .map_err(|e| Error::Storage(format!("cannot write to standard error: {e}")))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| Error::Storage(format!("cannot read the answer from standard input: {e}")))?;
    if is_yes(&answer) {
        Ok(())
    } else {
        Err(Error::Cancelled)
    }
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::is_yes;

    #[test]
    fn only_y_and_yes_say_yes() {
        for yes in ["y", "Y", "yes", "YES", " y \n", "yes\r\n"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yep", "ye", "1", "y y"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }
}
