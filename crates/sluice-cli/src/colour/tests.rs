use std::ffi::{OsStr, OsString};

use super::{ColorWhen, Colour, Terminals, peek, resolve};

const WHENS: [ColorWhen; 3] = [ColorWhen::Auto, ColorWhen::Always, ColorWhen::Never];

fn both(on: bool) -> Colour {
    Colour {
        stdout: on,
        stderr: on,
    }
}

/// Every combination of the flag, `NO_COLOR` (unset, empty, set) and each stream being a
/// terminal or not. The expectation is written from the rules and not from the code: a non-empty
/// `NO_COLOR` or `never` turns colour off, `always` turns it on for both streams, and `auto`
/// follows each stream.
#[test]
fn every_combination_of_flag_no_color_and_terminals() {
    let no_colors: [Option<&OsStr>; 3] = [None, Some(OsStr::new("")), Some(OsStr::new("1"))];
    for when in WHENS {
        for no_color in no_colors {
            for stdout in [false, true] {
                for stderr in [false, true] {
                    let no_color_set = no_color.is_some_and(|v| !v.is_empty());
                    let expected = if no_color_set || when == ColorWhen::Never {
                        both(false)
                    } else if when == ColorWhen::Always {
                        both(true)
                    } else {
                        Colour { stdout, stderr }
                    };
                    let got = resolve(when, no_color, Terminals { stdout, stderr });
                    assert_eq!(
                        got, expected,
                        "{when:?}, NO_COLOR {no_color:?}, stdout tty {stdout}, stderr tty {stderr}"
                    );
                }
            }
        }
    }
}

#[test]
fn no_color_wins_over_always_and_an_empty_one_does_not() {
    let terminals = Terminals {
        stdout: true,
        stderr: true,
    };
    let set = Some(OsStr::new("anything"));
    assert_eq!(resolve(ColorWhen::Always, set, terminals), both(false));
    let empty = Some(OsStr::new(""));
    assert_eq!(resolve(ColorWhen::Always, empty, terminals), both(true));
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn the_flag_is_found_in_the_raw_arguments() {
    let cases: [(&[&str], ColorWhen); 9] = [
        (&["sluice"], ColorWhen::Auto),
        (&["sluice", "--color", "always", "id"], ColorWhen::Always),
        (&["sluice", "id", "--color=never"], ColorWhen::Never),
        (&["sluice", "--color=ALWAYS"], ColorWhen::Always),
        // The last one wins, as it does in clap.
        (
            &["sluice", "--color", "always", "--color=never"],
            ColorWhen::Never,
        ),
        // After `--` it is not a flag.
        (&["sluice", "--", "--color=always"], ColorWhen::Auto),
        // A value that is not one of the three is clap's to report.
        (&["sluice", "--color", "purple"], ColorWhen::Auto),
        (&["sluice", "--color"], ColorWhen::Auto),
        // The program name is not an argument.
        (&["--color=always"], ColorWhen::Auto),
    ];
    for (list, expected) in cases {
        assert_eq!(peek(&args(list)), expected, "{list:?}");
    }
}
