use std::ffi::OsString;
use std::path::Path;

use crate::Args;

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
fn the_two_arguments_in_either_order_and_either_spelling() {
    let want = Args {
        open: true,
        config_path: "/etc/relay/open.toml".into(),
    };
    assert_eq!(
        parse(&["--open", "--config-path", "/etc/relay/open.toml"]),
        Ok(want.clone())
    );
    assert_eq!(
        parse(&["--config-path", "/etc/relay/open.toml", "--open"]),
        Ok(want.clone())
    );
    assert_eq!(
        parse(&["--config-path=/etc/relay/open.toml", "--open"]),
        Ok(want)
    );
    let closed = parse(&["--config-path", "/etc/relay/relay.toml"]).unwrap();
    assert!(!closed.open);
    assert_eq!(closed.config_path, Path::new("/etc/relay/relay.toml"));
}

#[test]
fn a_missing_config_path_is_refused_with_why() {
    refused(&[], "--config-path <file> is required");
    refused(&["--open"], "without a config file iroh-relay runs open");
    refused(&["--config-path"], "needs the path of the config file");
    refused(
        &["--open", "--config-path"],
        "needs the path of the config file",
    );
    refused(&["--config-path="], "needs the path of the config file");
    refused(&["--config-path", ""], "needs the path of the config file");
}

#[test]
fn dev_is_refused_alone_and_with_a_config() {
    refused(&["--dev"], "--dev is refused: it turns TLS off");
    refused(
        &["--dev", "--config-path", "/etc/relay/relay.toml"],
        "--dev is refused",
    );
    refused(
        &["--config-path", "/etc/relay/relay.toml", "--dev"],
        "--dev is refused",
    );
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
        &["--config-path", "a.toml", "--config-path=b.toml"],
        "--config-path is given twice",
    );
}

#[test]
fn a_path_that_looks_like_a_flag_is_still_the_path() {
    let args = parse(&["--config-path", "--open"]).unwrap();
    assert!(!args.open);
    assert_eq!(args.config_path, Path::new("--open"));
}
