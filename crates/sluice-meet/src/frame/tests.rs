use crate::frame::{
    ClientFrame, ErrorCode, FailReason, FrameError, MAX_FRAME, MAX_SEALED, Role, SPAKE_LEN,
    ServerFrame,
};

#[test]
fn every_client_frame_round_trips() {
    let cases = [
        ClientFrame::Claim,
        ClientFrame::Join(9999),
        ClientFrame::Spake(vec![7; SPAKE_LEN]),
        ClientFrame::Confirm([9; 32]),
        ClientFrame::Sealed(vec![1; 28]),
        ClientFrame::Fail(FailReason::ConfirmMismatch),
        ClientFrame::Fail(FailReason::SealFailed),
        ClientFrame::Done,
    ];
    for frame in cases {
        let bytes = frame.encode().unwrap();
        assert_eq!(ClientFrame::decode(&bytes), Ok(frame), "{bytes:?}");
    }
}

#[test]
fn every_server_frame_round_trips() {
    let cases = [
        ServerFrame::Claimed(1),
        ServerFrame::Joined(Role::Sender),
        ServerFrame::Joined(Role::Receiver),
        ServerFrame::Spake(vec![7; SPAKE_LEN]),
        ServerFrame::Confirm([9; 32]),
        ServerFrame::Sealed(vec![1; 28]),
        ServerFrame::Burned,
        ServerFrame::PeerClosed,
        ServerFrame::Done,
        ServerFrame::Error(ErrorCode::RateLimited),
    ];
    for frame in cases {
        let bytes = frame.encode().unwrap();
        assert_eq!(ServerFrame::decode(&bytes), Ok(frame), "{bytes:?}");
    }
}

#[test]
fn text_is_refused_before_its_bytes_are_read() {
    assert_eq!(
        ClientFrame::decode_message(true, b"\x01"),
        Err(FrameError::Text)
    );
    assert_eq!(
        ServerFrame::decode_message(true, b"\x88"),
        Err(FrameError::Text)
    );
}

#[test]
fn truncated_oversized_unknown_and_trailing_frames_are_refused() {
    assert_eq!(ClientFrame::decode(&[]), Err(FrameError::Truncated));
    assert_eq!(
        ClientFrame::decode(&[0x01, 0x00]),
        Err(FrameError::Trailing)
    );
    assert_eq!(
        ClientFrame::decode(&[0x04, 1, 2]),
        Err(FrameError::Truncated)
    );
    assert_eq!(ClientFrame::decode(&[0x99]), Err(FrameError::Unknown));
    assert_eq!(
        ClientFrame::decode(&vec![0x03; MAX_FRAME + 1]),
        Err(FrameError::TooLarge)
    );
    assert_eq!(ClientFrame::decode(&[0x05, 0]), Err(FrameError::BadSealed));
    assert_eq!(ClientFrame::decode(&[0x06, 3]), Err(FrameError::BadReason));
    assert_eq!(ServerFrame::decode(&[0x82, 2]), Err(FrameError::BadRole));
    assert_eq!(ServerFrame::decode(&[0x8f, 8]), Err(FrameError::BadCode));
}

#[test]
fn a_sealed_value_over_its_own_bound_is_refused() {
    let mut frame = vec![0x05];
    frame.extend_from_slice(&vec![0; MAX_SEALED + 1]);
    assert_eq!(ClientFrame::decode(&frame), Err(FrameError::TooLarge));
    assert_eq!(
        ClientFrame::Sealed(vec![0; MAX_SEALED + 1]).encode(),
        Err(FrameError::TooLarge)
    );
    assert_eq!(
        ClientFrame::Sealed(vec![0; 27]).encode(),
        Err(FrameError::BadSealed)
    );
}

#[test]
fn a_spake_message_has_exactly_33_bytes() {
    assert_eq!(
        ClientFrame::Spake(vec![0; 32]).encode(),
        Err(FrameError::Truncated)
    );
    assert_eq!(
        ClientFrame::Spake(vec![0; 34]).encode(),
        Err(FrameError::Trailing)
    );
    assert_eq!(ClientFrame::decode(&[0x03; 35]), Err(FrameError::Trailing));
}
