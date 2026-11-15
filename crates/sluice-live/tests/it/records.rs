//! The byte layouts of the four records: each length, each field's place, and a refusal of every
//! other length.

use sluice_live::{Error, Fault, Frame, LogClose, LogEntry, LogOpen};

fn open() -> LogOpen {
    LogOpen {
        stream_id: [1; 16],
        sender_key: [2; 32],
        segment_bytes: 0x0403_0201,
        started: 0x0c0b_0a09_0807_0605,
        sig: [3; 64],
    }
}

fn entry() -> LogEntry {
    LogEntry {
        seq: 0x0807_0605_0403_0201,
        segment_root: [4; 32],
        len: 0x0c0b_0a09,
        prev_entry_hash: [5; 32],
    }
}

fn close() -> LogClose {
    LogClose {
        final_root: [6; 32],
        total_len: 0x0807_0605_0403_0201,
        closed: 0x1817_1615_1413_1211,
        last_entry_hash: [7; 32],
        sig: [8; 64],
    }
}

#[test]
fn log_open_is_124_bytes_in_the_order_of_its_table() {
    let bytes = open().encode();
    assert_eq!(bytes.len(), 124);
    assert_eq!(bytes[..16], [1; 16]);
    assert_eq!(bytes[16..48], [2; 32]);
    assert_eq!(bytes[48..52], [1, 2, 3, 4], "segment_bytes, little-endian");
    assert_eq!(
        bytes[52..60],
        [5, 6, 7, 8, 9, 10, 11, 12],
        "started, little-endian"
    );
    assert_eq!(bytes[60..], [3; 64]);
    assert_eq!(open().hash(), *blake3::hash(&bytes[..60]).as_bytes());
    assert_eq!(LogOpen::decode(&bytes), Ok(open()));
}

#[test]
fn log_entry_is_76_bytes_and_a_frame_is_it_and_a_signature() {
    let bytes = entry().encode();
    assert_eq!(bytes.len(), 76);
    assert_eq!(bytes[..8], [1, 2, 3, 4, 5, 6, 7, 8], "seq, little-endian");
    assert_eq!(bytes[8..40], [4; 32]);
    assert_eq!(bytes[40..44], [9, 10, 11, 12], "len, little-endian");
    assert_eq!(bytes[44..], [5; 32]);
    assert_eq!(entry().hash(), *blake3::hash(&bytes).as_bytes());
    let frame = Frame {
        entry: entry(),
        sig: [9; 64],
    };
    let framed = frame.encode();
    assert_eq!(framed.len(), 140);
    assert_eq!(framed[..76], bytes);
    assert_eq!(framed[76..], [9; 64]);
    assert_eq!(Frame::decode(&framed), Ok(frame));
}

#[test]
fn log_close_is_144_bytes_in_the_order_of_its_table() {
    let bytes = close().encode();
    assert_eq!(bytes.len(), 144);
    assert_eq!(bytes[..32], [6; 32]);
    assert_eq!(
        bytes[32..40],
        [1, 2, 3, 4, 5, 6, 7, 8],
        "total_len, little-endian"
    );
    assert_eq!(
        bytes[40..48],
        [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18]
    );
    assert_eq!(bytes[48..80], [7; 32]);
    assert_eq!(bytes[80..], [8; 64]);
    assert_eq!(LogClose::decode(&bytes), Ok(close()));
}

#[test]
fn rule_1_every_other_length_is_refused() {
    let refused = Err(Error::Verify(Fault::Length));
    for len in [0, 1, 75, 76, 123, 125, 139, 141, 143, 145, 300] {
        let bytes = vec![0; len];
        assert_eq!(LogOpen::decode(&bytes).map(|_| ()), refused);
        if len != 76 {
            assert_eq!(LogEntry::decode(&bytes).map(|_| ()), refused);
        }
        assert_eq!(Frame::decode(&bytes).map(|_| ()), refused);
        assert_eq!(LogClose::decode(&bytes).map(|_| ()), refused);
    }
}

#[test]
fn any_bytes_of_the_right_length_are_one_record_with_one_spelling() {
    let bytes: Vec<u8> = (0..=255u8).cycle().take(144).collect();
    assert_eq!(
        LogOpen::decode(&bytes[..124]).map(|r| r.encode().to_vec()),
        Ok(bytes[..124].to_vec())
    );
    assert_eq!(
        Frame::decode(&bytes[..140]).map(|r| r.encode().to_vec()),
        Ok(bytes[..140].to_vec())
    );
    assert_eq!(
        LogClose::decode(&bytes).map(|r| r.encode().to_vec()),
        Ok(bytes.clone())
    );
}
