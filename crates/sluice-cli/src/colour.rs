//! Whether to use colour, decided once at the start and passed down.

use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal};

/// The value of `--color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorWhen {
    /// Colour a stream only when it is a terminal
    Auto,
    /// Colour always, unless `NO_COLOR` is set
    Always,
    /// Never colour
    Never,
}

/// Which of the two output streams gets colour. They are separate because `sluice id | clip` has
/// a terminal on stderr and not on stdout, and each stream is judged on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colour {
    pub stdout: bool,
    pub stderr: bool,
}

/// Whether each output stream is a terminal.
#[derive(Clone, Copy, Debug)]
pub struct Terminals {
    pub stdout: bool,
    pub stderr: bool,
}

/// What the process was started in, read once. The environment is not read again afterwards, so
/// everything that prints agrees.
pub struct Environment {
    no_color: Option<OsString>,
    terminals: Terminals,
}

impl Environment {
    pub fn read() -> Self {
        Self {
            no_color: std::env::var_os("NO_COLOR"),
            terminals: Terminals {
                stdout: io::stdout().is_terminal(),
                stderr: io::stderr().is_terminal(),
            },
        }
    }

    pub fn colour(&self, when: ColorWhen) -> Colour {
        resolve(when, self.no_color.as_deref(), self.terminals)
    }
}

/// `NO_COLOR` set to anything but the empty string turns colour off and wins over
/// `--color always`, as the no-color.org convention says. Otherwise `always` and `never` mean what
/// they say, and `auto` is colour on a terminal.
pub fn resolve(when: ColorWhen, no_color: Option<&OsStr>, terminals: Terminals) -> Colour {
    if no_color.is_some_and(|value| !value.is_empty()) {
        return Colour {
            stdout: false,
            stderr: false,
        };
    }
    match when {
        ColorWhen::Always => Colour {
            stdout: true,
            stderr: true,
        },
        ColorWhen::Never => Colour {
            stdout: false,
            stderr: false,
        },
        ColorWhen::Auto => Colour {
            stdout: terminals.stdout,
            stderr: terminals.stderr,
        },
    }
}

/// The last `--color` in `args`, for printing a parse failure. clap prints help and errors while
/// it parses, before there is a parsed value to read, so the flag is looked for in the raw
/// arguments. A value that is not one of the three is left for clap to report.
pub fn peek(args: &[OsString]) -> ColorWhen {
    use clap::ValueEnum;
    let mut found = ColorWhen::Auto;
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else { continue };
        let value = if arg == "--" {
            break;
        } else if arg == "--color" {
            args.next().and_then(|v| v.to_str())
        } else {
            arg.strip_prefix("--color=")
        };
        if let Some(parsed) = value.and_then(|v| ColorWhen::from_str(v, true).ok()) {
            found = parsed;
        }
    }
    found
}

#[cfg(test)]
mod tests;
