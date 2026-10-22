use std::ffi::OsString;
use std::path::Path;

use crate::{Args, Mode};

fn parse(args: &[&str]) -> Result<Args, String> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    Args::parse(&args).map_err(|refusal| refusal.to_string())
}

fn refused(args: &[&str], says: &str) {
    let message = parse(args).expect_err("refused");
    assert!(message.contains(says), "{args:?}: {message}");
    assert!(!message.contains('\n'), "one line: {message}");
}

#[test]
fn the_arguments_in_either_order_and_either_spelling() {
    for (flag, mode) in [("--open", Mode::Open), ("--rendezvous", Mode::Rendezvous)] {
        let want = Args {
            mode,
            config_path: "/etc/relay/a.toml".into(),
        };
        assert_eq!(
            parse(&[flag, "--config-path", "/etc/relay/a.toml"]),
            Ok(want.clone())
        );
        assert_eq!(
            parse(&["--config-path", "/etc/relay/a.toml", flag]),
            Ok(want.clone())
        );
        assert_eq!(parse(&["--config-path=/etc/relay/a.toml", flag]), Ok(want));
    }
    let closed = parse(&["--config-path", "/etc/relay/relay.toml"]).unwrap();
    assert_eq!(closed.mode, Mode::Closed);
    assert_eq!(closed.config_path, Path::new("/etc/relay/relay.toml"));
}

#[test]
fn a_missing_config_path_is_refused_with_why() {
    refused(&[], "--config-path <file> is required");
    refused(&["--open"], "without a config file iroh-relay runs open");
    refused(&["--rendezvous"], "--config-path <file> is required");
    refused(&["--config-path"], "needs the path of the config file");
    refused(
        &["--open", "--config-path"],
        "needs the path of the config file",
    );
    refused(&["--config-path="], "needs the path of the config file");
    refused(&["--config-path", ""], "needs the path of the config file");
}

#[test]
fn dev_is_refused_alone_and_with_a_config_in_every_mode() {
    refused(&["--dev"], "--dev is refused: it turns TLS off");
    for flags in [&[][..], &["--open"], &["--rendezvous"]] {
        let mut args = flags.to_vec();
        args.extend(["--dev", "--config-path", "/etc/relay/relay.toml"]);
        refused(&args, "--dev is refused");
        let mut args = flags.to_vec();
        args.extend(["--config-path", "/etc/relay/relay.toml", "--dev"]);
        refused(&args, "--dev is refused");
    }
}

#[test]
fn every_other_argument_is_refused_and_named() {
    for arg in [
        "--help",
        "-c",
        "-h",
        "--version",
        "--config",
        "relay.toml",
        "--open=yes",
        "--rendezvous=yes",
        "-",
    ] {
        refused(
            &[arg, "--config-path", "/etc/relay/relay.toml"],
            &format!("`{arg}`"),
        );
    }
    // iroh-relay's own short flag for the config path is not one of the guard's.
    refused(&["-c", "/etc/relay/relay.toml"], "unknown argument `-c`");
}

#[test]
fn a_repeated_argument_is_refused() {
    refused(
        &["--open", "--open", "--config-path", "a.toml"],
        "--open is given twice",
    );
    refused(
        &["--rendezvous", "--config-path", "a.toml", "--rendezvous"],
        "--rendezvous is given twice",
    );
    refused(
        &["--config-path", "a.toml", "--config-path=b.toml"],
        "--config-path is given twice",
    );
}

#[test]
fn open_and_rendezvous_together_are_refused_in_either_order() {
    for args in [
        ["--open", "--rendezvous", "--config-path", "a.toml"],
        ["--rendezvous", "--config-path", "a.toml", "--open"],
    ] {
        refused(&args, "--open and --rendezvous are given together");
    }
}

#[test]
fn a_path_that_looks_like_a_flag_is_still_the_path() {
    let args = parse(&["--config-path", "--open"]).unwrap();
    assert_eq!(args.mode, Mode::Closed);
    assert_eq!(args.config_path, Path::new("--open"));
}
