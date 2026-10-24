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
            behind_proxy: false,
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
    for arg in ["--help", "--version", "--config"] {
        refused(
            &[arg, "--config-path", "/etc/relay/relay.toml"],
            &format!("`{arg}`"),
        );
    }
}

/// An argument that does not look like a flag may be a token pasted in the wrong place, so it is
/// not printed, and its position says which one it is.
#[test]
fn an_argument_that_is_not_a_flag_is_not_printed() {
    let base64url = "c2VjcmV0LXRva2Vu-_Tg9Zk";
    let hex = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    let long = format!("--{}", "a".repeat(33));
    for arg in [
        base64url,
        hex,
        "relay.toml",
        "--open=yes",
        "--rendezvous=yes",
        "-c",
        "-h",
        "-",
        "--Open",
        "--a1",
        &long,
        "--",
    ] {
        let args = ["--open", arg, "--config-path", "/etc/relay/relay.toml"];
        let message = parse(&args).expect_err("refused");
        assert!(
            message.contains("an argument at position 2 is unknown"),
            "{arg}: {message}"
        );
        // The whole message, so that nothing of the argument is in it.
        assert_eq!(
            message,
            "an argument at position 2 is unknown: the guard takes only \
             [--open | --rendezvous | --behind-proxy] --config-path <file>"
        );
    }
    // The position counts the path after --config-path, so it is the place on the command line.
    let args = ["--config-path", "a.toml", base64url];
    let message = parse(&args).expect_err("refused");
    assert!(
        message.contains("an argument at position 3 is unknown"),
        "{message}"
    );
    // iroh-relay's own short flag for the config path is not one of the guard's.
    refused(
        &["-c", "/etc/relay/relay.toml"],
        "an argument at position 1",
    );
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

#[test]
fn behind_proxy_is_parsed_once_and_only_for_a_relay() {
    let args = parse(&["--behind-proxy", "--config-path", "a.toml"]).unwrap();
    assert!(args.behind_proxy);
    assert_eq!(args.mode, Mode::Closed);
    let open = parse(&["--behind-proxy", "--open", "--config-path", "a.toml"]).unwrap();
    assert!(open.behind_proxy);
    assert_eq!(open.mode, Mode::Open);
    refused(
        &[
            "--behind-proxy",
            "--behind-proxy",
            "--config-path",
            "a.toml",
        ],
        "--behind-proxy is given twice",
    );
    // It is for a relay whose config has no [tls]; a rendezvous server relays nothing.
    refused(
        &["--rendezvous", "--behind-proxy", "--config-path", "a.toml"],
        "--behind-proxy is for a relay behind a proxy",
    );
    refused(
        &["--behind-proxy", "--rendezvous", "--config-path", "a.toml"],
        "--behind-proxy is for a relay behind a proxy",
    );
}
