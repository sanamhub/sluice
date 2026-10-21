//! Helpers for writing manifest bytes by hand, so that a test puts one wrong field in an
//! otherwise valid manifest. They do not call the encoder, which is what the tests check.

use crate::Error;
use crate::manifest::{Entry, Kind, Limits, Loc, Manifest, link::LinkTarget, path::ManifestPath};

pub(super) fn varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

pub(super) fn text(s: &str) -> Vec<u8> {
    let mut out = varint(s.len() as u64);
    out.extend_from_slice(s.as_bytes());
    out
}

/// The bytes of a loc: `Blob` is tag 0 and a root, `Packed` is tag 1, `None` is tag 2.
pub(super) fn blob(fill: u8) -> Vec<u8> {
    [varint(0), vec![fill; 32]].concat()
}

pub(super) fn packed(pack_index: u32, offset: u64, len: u64) -> Vec<u8> {
    [
        varint(1),
        varint(u64::from(pack_index)),
        varint(offset),
        varint(len),
    ]
    .concat()
}

pub(super) fn no_loc() -> Vec<u8> {
    varint(2)
}

pub(super) fn no_link() -> Vec<u8> {
    vec![0]
}

pub(super) fn link(target: &str) -> Vec<u8> {
    [vec![1], text(target)].concat()
}

/// One entry, each field as the bytes to write for it.
pub(super) struct Raw {
    pub path: Vec<u8>,
    pub kind: Vec<u8>,
    pub size: Vec<u8>,
    pub mtime: Vec<u8>,
    pub mode: Vec<u8>,
    pub loc: Vec<u8>,
    pub link: Vec<u8>,
}

impl Raw {
    pub(super) fn bytes(&self) -> Vec<u8> {
        [
            self.path.as_slice(),
            &self.kind,
            &self.size,
            &self.mtime,
            &self.mode,
            &self.loc,
            &self.link,
        ]
        .concat()
    }
}

pub(super) fn file(path: &str) -> Raw {
    Raw {
        path: text(path),
        kind: vec![0],
        size: varint(5),
        mtime: varint(1_700_000_000_000),
        mode: varint(0o644),
        loc: blob(0xab),
        link: no_link(),
    }
}

pub(super) fn dir(path: &str) -> Raw {
    Raw {
        path: text(path),
        kind: vec![1],
        size: varint(0),
        mtime: varint(1_700_000_000_000),
        mode: varint(0o755),
        loc: no_loc(),
        link: no_link(),
    }
}

pub(super) fn symlink(path: &str, target: &str) -> Raw {
    Raw {
        path: text(path),
        kind: vec![2],
        size: varint(0),
        mtime: varint(0),
        mode: varint(0),
        loc: no_loc(),
        link: link(target),
    }
}

/// A manifest of version 1 with these entries and no packs.
pub(super) fn manifest(entries: &[Raw]) -> Vec<u8> {
    with_packs(entries, &[])
}

/// A manifest of version 1 with these entries and these packs, each `(root fill, size)`.
pub(super) fn with_packs(entries: &[Raw], packs: &[(u8, u64)]) -> Vec<u8> {
    let mut out = [varint(1), varint(entries.len() as u64)].concat();
    for entry in entries {
        out.extend(entry.bytes());
    }
    out.extend(varint(packs.len() as u64));
    for (fill, size) in packs {
        out.extend([vec![*fill; 32], varint(*size)].concat());
    }
    out
}

pub(super) fn decode(bytes: &[u8]) -> Result<Manifest, Error> {
    Manifest::decode(bytes, Limits::default())
}

/// The rule a decode error names: `manifest rule 9 (path rule 4): ...` is `(9, Some(4))`.
pub(super) fn rule_of(err: &Error) -> (u8, Option<u8>) {
    let Error::Policy(message) = err else {
        panic!("not a policy error: {err}");
    };
    let rest = message
        .strip_prefix("manifest rule ")
        .unwrap_or_else(|| panic!("not a manifest rule: {message}"));
    let (number, rest) = rest
        .split_once(|c: char| !c.is_ascii_digit())
        .expect("a rule");
    let number: u8 = number.parse().expect("digits");
    let sub = rest
        .strip_prefix('(')
        .and_then(|r| r.split_once(')'))
        .and_then(|(inside, _)| inside.rsplit(' ').next())
        .map(|n| n.parse().expect("a sub rule number"));
    (number, sub)
}

/// The rule that `bytes` break, or `None` if they decode.
pub(super) fn broken(bytes: &[u8]) -> Option<(u8, Option<u8>)> {
    decode(bytes).err().map(|e| rule_of(&e))
}

pub(super) fn path(text: &str) -> ManifestPath {
    ManifestPath::parse(text).expect("a path that passes the rules")
}

/// A typed entry, for the tests of `Manifest::new`.
pub(super) fn typed(path_text: &str, kind: Kind, loc: Loc, size: u64) -> Entry {
    Entry {
        path: path(path_text),
        kind,
        size,
        mtime_ms: 0,
        mode: 0o644,
        loc,
        link_target: None,
    }
}

pub(super) fn typed_file(path_text: &str) -> Entry {
    typed(path_text, Kind::File, Loc::Blob { root: [1; 32] }, 3)
}

pub(super) fn typed_dir(path_text: &str) -> Entry {
    typed(path_text, Kind::Dir, Loc::None, 0)
}

pub(super) fn typed_link(path_text: &str, target: &str) -> Entry {
    Entry {
        link_target: Some(LinkTarget::parse(target).expect("a target")),
        ..typed(path_text, Kind::Symlink, Loc::None, 0)
    }
}
