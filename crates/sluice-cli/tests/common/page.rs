//! A daemon with its page listener, and the requests the tests of the page make to it: by hand
//! over a socket, so every header is exactly what a test says.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

use super::daemon::{Running, start_with};

pub struct Page {
    pub _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub port: u16,
    pub key: String,
    pub daemon: Running,
}

pub fn page() -> Page {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("files");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("payload.bin"), super::data(300 * 1024, 5)).unwrap();
    std::fs::create_dir(root.join("docs")).unwrap();
    let daemon = start_with(&home, &[&root], "info", &["--ui"]);
    let text = std::fs::read_to_string(home.join("ui.json")).expect("the port file");
    let file: Value = serde_json::from_str(&text).unwrap();
    Page {
        port: file["port"].as_u64().unwrap() as u16,
        key: file["key"].as_str().unwrap().to_string(),
        daemon,
        root,
        _dir: dir,
    }
}

/// A request written by hand, so every header is exactly what the test says, and its answer: the
/// status, the headers in lower case, and the body.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn cors(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n.starts_with("access-control"))
            .map(|(n, _)| n.as_str())
            .collect()
    }
}

pub fn parse(raw: &str) -> Answer {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_lowercase(), v.trim().to_string()))
        .collect();
    Answer {
        status,
        headers,
        body: body.to_string(),
    }
}

pub fn raw(port: u16, request: &str) -> Answer {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut text = Vec::new();
    // `Connection: close` ends the answer; a body that follows is read to the end.
    let _ = stream.read_to_end(&mut text);
    parse(&String::from_utf8_lossy(&text))
}

/// `GET path` with the headers given, and the right host unless one is given.
pub fn get(page: &Page, path: &str, extra: &[(&str, &str)]) -> Answer {
    let mut request = format!("GET {path} HTTP/1.1\r\nConnection: close\r\n");
    if !extra.iter().any(|(n, _)| n.eq_ignore_ascii_case("host")) {
        request.push_str(&format!("Host: 127.0.0.1:{}\r\n", page.port));
    }
    for (name, value) in extra {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    raw(page.port, &(request + "\r\n"))
}

pub fn authorized(page: &Page) -> (String, String) {
    ("Authorization".to_string(), format!("Bearer {}", page.key))
}

pub fn post(
    page: &Page,
    path: &str,
    content_type: &str,
    body: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let (a, b) = authorized(page);
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nConnection: close\r\nHost: 127.0.0.1:{}\r\n{a}: {b}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
        page.port,
        body.len()
    );
    for (name, value) in extra {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    raw(page.port, &(request + "\r\n" + body))
}

/// The body of an answer as JSON, after a check of the status.
pub fn json_of(answer: &Answer, want: u16) -> Value {
    assert_eq!(answer.status, want, "{}", answer.body);
    serde_json::from_str(&answer.body).unwrap()
}

/// The lines of the events stream that are JSON objects, read on a thread of their own. The chunk
/// framing between them is skipped.
pub fn watch(page: &Page) -> mpsc::Receiver<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", page.port)).unwrap();
    let request = format!(
        "GET /api/v1/events HTTP/1.1{n}Host: 127.0.0.1:{}{n}Authorization: Bearer {}{n}{n}",
        page.port,
        page.key,
        n = "\r\n"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if line.starts_with('{')
                && let Ok(event) = serde_json::from_str(&line)
                && tx.send(event).is_err()
            {
                break;
            }
        }
    });
    rx
}
