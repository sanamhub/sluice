use std::net::Ipv4Addr;

use iroh::{
    Endpoint, RelayMode,
    endpoint::{Connection, VarInt, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use serde_json::Value;
use sluice_core::{
    Error,
    engine::{ALPN, Hello, ReceiveOptions, encode_hello, receive_live},
    manifest::Limits,
    ticket::{Body, Ticket},
};

use super::{Failure, Format, bytes_text, failure_line, summary};

#[test]
fn sizes_use_binary_units_and_do_not_divide_by_zero() {
    assert_eq!(bytes_text(0), "0 bytes");
    assert_eq!(bytes_text(1536), "1.5 KiB");
    assert_eq!(bytes_text(100 << 20), "100.0 MiB");
}

/// The one line `inspect` and a receive both print: the counts a plural needs, and the size.
#[test]
fn the_summary_line_counts_what_it_should() {
    assert_eq!(summary(1, 0, 0, 0), "1 file, 0 bytes");
    assert_eq!(summary(2, 0, 0, 1536), "2 files, 1.5 KiB");
    assert_eq!(summary(3, 2, 0, 1536), "3 files, 2 folders, 1.5 KiB");
    assert_eq!(summary(3, 1, 1, 1536), "3 files, 1 folder, 1 link, 1.5 KiB");
}

#[test]
fn a_json_failure_is_one_valid_line_whatever_the_message_holds() {
    let messages = [
        "plain",
        "a \"quoted\" word",
        "back\\slash and C:\\Users\\x",
        "two\nlines\r\nand a tab\t",
        "control \u{1} \u{7} \u{1f} and \u{7f}",
        "unicode \u{e9} \u{1f600} \u{2028}",
        "",
    ];
    for text in messages {
        let line = failure_line(Format::Json, &Failure::Error(Error::Policy(text.into())));
        assert!(line.ends_with('\n'), "{line:?}");
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["error"]["kind"], "policy");
        assert_eq!(parsed["error"]["message"], text);
        assert_eq!(parsed["error"].as_object().unwrap().len(), 2);
    }
}

#[test]
fn a_text_failure_is_sluice_a_colon_and_the_message() {
    let line = failure_line(
        Format::Human,
        &Failure::Error(Error::Storage("disk full".into())),
    );
    assert_eq!(line, "sluice: storage error: disk full\n");
}

/// A failure can carry text the peer chose, such as the reason it closed the connection with.
/// The terminal must not obey a character in it: it is written as its escape, and the line stays
/// one line. A backslash is doubled, so a literal `\u{202e}` cannot be read as the escape.
#[test]
fn a_failure_line_escapes_the_text_a_stranger_wrote() {
    for message in [
        "the transfer stopped: \u{1b}[2Jreport\u{202e}fdp.exe",
        "a line separator \u{2028} and a paragraph separator \u{2029}",
        "two\nlines",
        r"a literal \u{202e} and a Windows path C:\Users\x",
    ] {
        let line = failure_line(
            Format::Human,
            &Failure::Error(Error::Network(message.into())),
        );
        assert!(line.ends_with('\n'), "{line:?}");
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");
        for character in ['\u{1b}', '\u{202e}', '\u{2028}', '\u{2029}'] {
            assert!(!line.contains(character), "{character:?} in {line:?}");
        }
        assert!(line.starts_with("sluice: network error: "), "{line:?}");
    }
    let line = failure_line(
        Format::Human,
        &Failure::Error(Error::Network("a\u{1b}[2Jb\u{202e}".into())),
    );
    assert_eq!(line, "sluice: network error: a\\u{1b}[2Jb\\u{202e}\n");
    let line = failure_line(
        Format::Human,
        &Failure::Error(Error::Network(r"literal \u{202e}".into())),
    );
    assert_eq!(line, "sluice: network error: literal \\\\u{202e}\n");
}

/// A path on Windows keeps one backslash per separator: a backslash is doubled only where it
/// would otherwise start an escape, so a reader can read the path the sender wrote.
#[test]
fn a_path_in_a_failure_line_keeps_the_backslashes_it_was_written_with() {
    let line = failure_line(
        Format::Human,
        &Failure::Error(Error::Storage(
            r"C:\Users\me\out\tree already exists".into(),
        )),
    );
    assert_eq!(
        line,
        "sluice: storage error: C:\\Users\\me\\out\\tree already exists\n"
    );
    // A message a peer wrote can hold the text of an escape, which is the one case that is
    // doubled, so it cannot be read as the escape itself.
    let line = failure_line(
        Format::Human,
        &Failure::Error(Error::Network(
            r"the name is C:\Users\me\u{202e}.txt".into(),
        )),
    );
    assert_eq!(
        line,
        "sluice: network error: the name is C:\\Users\\me\\\\u{202e}.txt\n"
    );
}

/// A listener that greets the dialer and then closes the connection with a reason of its own, which
/// is what a sender that is not this program can do.
#[derive(Debug)]
struct Closer(Vec<u8>);

impl ProtocolHandler for Closer {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let (mut send, _recv) = conn.accept_bi().await?;
        send.write_all(&encode_hello(Hello::ours()))
            .await
            .map_err(AcceptError::from_err)?;
        conn.close(VarInt::from_u32(8), &self.0);
        Ok(())
    }
}

/// A peer chooses the reason it closes a connection with, and a QUIC close carries that reason to
/// the other side, where an error's `Display` prints it. This is the whole path: a real connection
/// on loopback, closed with an escape sequence and a bidirectional character, and the line the
/// receiving side prints. The terminal must be given their escapes, not them.
#[tokio::test]
async fn a_close_reason_from_a_peer_is_escaped_in_the_failure_line() {
    let reason = "\u{1b}[2Jreport\u{202e}fdp.exe";
    let dir = tempfile::tempdir().unwrap();
    let (router, ticket, store, options) = closed_by_a_peer(reason, dir.path()).await;
    let (events, _unread) = tokio::sync::mpsc::unbounded_channel();
    let error = receive_live(&ticket, &options, &store, &events)
        .await
        .unwrap_err();
    router.shutdown().await.unwrap();

    // The peer's text did reach the error the receiving side made.
    assert!(error.to_string().contains('\u{1b}'), "{error}");
    let line = failure_line(Format::Human, &Failure::Error(error));
    assert!(line.contains("\\u{1b}[2J"), "{line:?}");
    assert!(line.contains("\\u{202e}fdp.exe"), "{line:?}");
    assert!(!line.contains('\u{1b}'), "{line:?}");
    assert!(!line.contains('\u{202e}'), "{line:?}");
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
}

/// The live ticket, store and options that reach [`Closer`], which closes with `reason`, listening
/// on loopback.
async fn closed_by_a_peer(
    reason: &str,
    dir: &std::path::Path,
) -> (Router, String, std::path::PathBuf, ReceiveOptions) {
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let router = Router::builder(endpoint.clone())
        .accept(ALPN, Closer(reason.as_bytes().to_vec()))
        .spawn();
    let id = *endpoint.id().as_bytes();
    let direct = endpoint
        .bound_sockets()
        .into_iter()
        .find(|addr| addr.is_ipv4())
        .expect("no IPv4 socket");
    let ticket = Ticket::new(
        id,
        vec![direct],
        None,
        None,
        Body::Live {
            stream_id: [7; 16],
            sender_key: id,
        },
    )
    .unwrap();
    let store = dir.join("store");
    std::fs::create_dir(&store).unwrap();
    let options = ReceiveOptions {
        out: dir.join("live.bin"),
        force: false,
        no_times: false,
        links: false,
        allow_hidden: false,
        limits: Limits::default(),
        free_bytes: None,
        max_size: None,
        congestion: sluice_core::engine::Congestion::Cubic,
        rate: None,
        write_limit: None,
        max_network_failures: 1,
        include: None,
    };
    (router, ticket.encode(), store, options)
}
