//! `sluice ui`: opens the page a daemon started with `--ui` serves, with its key in the address.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

use clap::Args;
use sluice_core::{Error, Home};

use crate::output::Output;
use crate::ui::PORT_FILE;

/// Opens the page of the daemon that is running.
#[derive(Args)]
pub struct UiArgs {
    /// Print the address and do not open a browser. The address holds the key, so anyone who can
    /// read it can control the daemon
    #[arg(long)]
    pub print: bool,
}

/// What it says when there is nothing to open.
const NO_DAEMON: &str = "no daemon with the page is running; start one with \
`sluice daemon --ui --root FOLDER`";

pub fn run(args: &UiArgs, output: &Output) -> Result<u8, Error> {
    let url = address(&Home::locate()?)?;
    if args.print {
        output.write_stderr(
            "sluice: this address holds the key; anyone who can read it can control the daemon\n",
        );
        output.write_stdout(&format!("{url}\n"))?;
        return Ok(0);
    }
    open(&url)?;
    output.write_stdout("opened the page in the default browser\n")?;
    Ok(0)
}

/// The address of the page, from the file the daemon wrote in the home, once something answers on
/// its port.
pub fn address(home: &std::path::Path) -> Result<String, Error> {
    let text = fs::read_to_string(home.join(PORT_FILE))
        .map_err(|_| Error::Network(NO_DAEMON.to_string()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| Error::Network(NO_DAEMON.to_string()))?;
    let port = value["port"].as_u64().and_then(|p| u16::try_from(p).ok());
    let key = value["key"].as_str();
    let (Some(port), Some(key)) = (port, key) else {
        return Err(Error::Network(NO_DAEMON.to_string()));
    };
    // The key goes into an address that the system's opener is given. A key the daemon wrote is 64
    // hex characters, and nothing else is passed on.
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Network(NO_DAEMON.to_string()));
    }
    let place = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    // A file left by a daemon that is gone names a port nobody listens on.
    TcpStream::connect_timeout(&place, Duration::from_secs(2))
        .map_err(|_| Error::Network(NO_DAEMON.to_string()))?;
    Ok(format!("http://127.0.0.1:{port}/#key={key}"))
}

/// Opens `url` with the platform's own opener. The address is one argument and no shell reads it.
fn open(url: &str) -> Result<(), Error> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command.arg(url).spawn().map(drop).map_err(|e| {
        Error::Storage(format!(
            "cannot open a browser: {e}; run `sluice ui --print` and open the address yourself"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    /// A key of the form the daemon writes.
    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn with_no_file_the_message_says_how_to_start_a_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let Err(Error::Network(text)) = address(dir.path()) else {
            panic!("no daemon, no address");
        };
        assert!(text.contains("sluice daemon --ui"), "{text}");
    }

    #[test]
    fn a_file_whose_port_nobody_listens_on_is_not_a_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let free = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        fs::write(
            dir.path().join(PORT_FILE),
            format!(r#"{{"port":{free},"key":"k"}}"#),
        )
        .unwrap();
        assert!(matches!(address(dir.path()), Err(Error::Network(_))));
    }

    #[test]
    fn a_running_listener_gives_an_address_with_the_key_in_the_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        fs::write(
            dir.path().join(PORT_FILE),
            format!(r#"{{"port":{port},"key":"{KEY}"}}"#),
        )
        .unwrap();
        assert_eq!(
            address(dir.path()).unwrap(),
            format!("http://127.0.0.1:{port}/#key={KEY}")
        );
    }

    #[test]
    fn a_file_that_is_not_json_or_has_no_key_is_not_a_daemon() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            "not json",
            r#"{"port":1}"#,
            r#"{"key":"k"}"#,
            r#"{"port":99999,"key":"k"}"#,
        ] {
            fs::write(dir.path().join(PORT_FILE), text).unwrap();
            assert!(
                matches!(address(dir.path()), Err(Error::Network(_))),
                "{text}"
            );
        }
    }

    /// The opener is handed the address, so a key that is not the 64 hex characters the daemon
    /// writes is not passed on, whatever answers on the port.
    #[test]
    fn a_key_that_is_not_64_hex_characters_gives_no_address() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let long = format!("{KEY}0");
        let odd = KEY.replace('f', "&");
        for key in ["abc123", long.as_str(), odd.as_str(), ""] {
            fs::write(
                dir.path().join(PORT_FILE),
                serde_json::json!({ "port": port, "key": key }).to_string(),
            )
            .unwrap();
            assert!(
                matches!(address(dir.path()), Err(Error::Network(_))),
                "{key}"
            );
        }
    }
}
