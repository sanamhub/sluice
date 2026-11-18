use std::net::{IpAddr, Ipv4Addr};

use super::*;
use crate::frame::FailReason;
use crate::meet::{Config, MeetingPlace};

const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));

/// One event list's frames, in order.
fn sent(events: Vec<ClientEvent>) -> Vec<ClientFrame> {
    events
        .into_iter()
        .filter_map(|event| match event {
            ClientEvent::Send(frame) => Some(frame),
            _ => None,
        })
        .collect()
}

fn one(events: Vec<ClientEvent>) -> ClientFrame {
    let mut frames = sent(events);
    assert_eq!(frames.len(), 1, "{frames:?}");
    frames.remove(0)
}

#[test]
fn the_right_words_deliver_the_ticket_and_both_sides_finish() {
    let ticket = "sluice1abcdefghjkmnpqrstvwxyz";
    let mut place = MeetingPlace::new(Config::new(100, 10, 600).unwrap());
    let sconn = place.connect(IP, 0).unwrap();
    let rconn = place.connect(IP, 0).unwrap();
    let (mut sender, claim) = Sender::new([1247, 1936], ticket).unwrap();
    let (mut receiver, join) = Receiver::new(Code::parse("1-orbit-velvet").unwrap());

    let out = place.receive(sconn, claim, 0).unwrap();
    assert_eq!(out[0].frame, ServerFrame::Claimed(1));
    assert!(matches!(
        sender.on_frame(out[0].frame.clone()).unwrap().as_slice(),
        [ClientEvent::Claimed]
    ));
    assert_eq!(sender.code().unwrap().text(), "1-orbit-velvet");

    let out = place.receive(rconn, join, 0).unwrap();
    assert!(to(&out, sconn, ServerFrame::Joined(Role::Sender)));
    assert!(to(&out, rconn, ServerFrame::Joined(Role::Receiver)));
    let a = one(sender.on_frame(ServerFrame::Joined(Role::Sender)).unwrap());
    let b = one(receiver
        .on_frame(ServerFrame::Joined(Role::Receiver))
        .unwrap());
    let ClientFrame::Spake(a) = a else {
        panic!("{a:?}")
    };
    let ClientFrame::Spake(b) = b else {
        panic!("{b:?}")
    };

    // Each side gets the other's SPAKE2 message and answers with its confirmation tag.
    let out = place
        .receive(sconn, ClientFrame::Spake(a.clone()), 1)
        .unwrap();
    assert!(to(&out, rconn, ServerFrame::Spake(a)));
    let receiver_confirm = one(receiver.on_frame(out[0].frame.clone()).unwrap());
    let out = place
        .receive(rconn, ClientFrame::Spake(b.clone()), 1)
        .unwrap();
    assert!(to(&out, sconn, ServerFrame::Spake(b)));
    let sender_confirm = one(sender.on_frame(out[0].frame.clone()).unwrap());
    let (ClientFrame::Confirm(receiver_confirm), ClientFrame::Confirm(sender_confirm)) =
        (receiver_confirm, sender_confirm)
    else {
        panic!("a side did not answer with its confirmation")
    };

    // The sender checks the receiver's tag, seals, and waits for Done.
    let out = place
        .receive(sconn, ClientFrame::Confirm(sender_confirm), 2)
        .unwrap();
    let events = receiver.on_frame(out[0].frame.clone()).unwrap();
    assert!(sent(events).is_empty(), "the receiver waits for Sealed");
    let out = place
        .receive(rconn, ClientFrame::Confirm(receiver_confirm), 2)
        .unwrap();
    let sealed = one(sender.on_frame(out[0].frame.clone()).unwrap());
    let ClientFrame::Sealed(sealed) = sealed else {
        panic!("{sealed:?}")
    };
    let out = place
        .receive(sconn, ClientFrame::Sealed(sealed), 3)
        .unwrap();
    let events = receiver.on_frame(out[0].frame.clone()).unwrap();
    let done = one(events);
    assert_eq!(done, ClientFrame::Done);
    assert!(to(
        &place.receive(rconn, done, 3).unwrap(),
        sconn,
        ServerFrame::Done
    ));
    assert!(matches!(
        sender.on_frame(ServerFrame::Done).unwrap().as_slice(),
        [ClientEvent::Done]
    ));
    assert_eq!(place.plates(), 0);
}

#[test]
fn a_wrong_word_pair_burns_and_both_sides_report_it() {
    let receiver_code = Code::parse("1-orbit-vendor").unwrap();
    let mut place = MeetingPlace::new(Config::new(100, 10, 600).unwrap());
    let sconn = place.connect(IP, 0).unwrap();
    let rconn = place.connect(IP, 0).unwrap();
    let (mut sender, claim) = Sender::new([1247, 1936], "sluice1abc").unwrap();
    let (mut receiver, join) = Receiver::new(receiver_code);

    place.receive(sconn, claim, 0).unwrap();
    sender.on_frame(ServerFrame::Claimed(1)).unwrap();
    let _out = place.receive(rconn, join, 0).unwrap();
    let a = one(sender.on_frame(ServerFrame::Joined(Role::Sender)).unwrap());
    let b = one(receiver
        .on_frame(ServerFrame::Joined(Role::Receiver))
        .unwrap());
    let ClientFrame::Spake(a) = a else { panic!() };
    let ClientFrame::Spake(b) = b else { panic!() };
    let out = place.receive(sconn, ClientFrame::Spake(a), 1).unwrap();
    let r_tag = one(receiver.on_frame(out[0].frame.clone()).unwrap());
    let out = place.receive(rconn, ClientFrame::Spake(b), 1).unwrap();
    let s_tag = one(sender.on_frame(out[0].frame.clone()).unwrap());
    let (ClientFrame::Confirm(r_tag), ClientFrame::Confirm(_s_tag)) = (r_tag, s_tag) else {
        panic!()
    };

    // The receiver's tag reaches the sender, whose check fails. The sender sends Fail and the
    // meeting place burns the plate; the receiver learns it from Burned.
    let out = place
        .receive(rconn, ClientFrame::Confirm(r_tag), 2)
        .unwrap();
    let events = sender.on_frame(out[0].frame.clone()).unwrap();
    assert_eq!(
        sent(events),
        vec![ClientFrame::Fail(FailReason::ConfirmMismatch)]
    );
    assert!(matches!(
        sender.on_frame(ServerFrame::Burned).unwrap().as_slice(),
        [ClientEvent::Burned]
    ));
    let out = place
        .receive(sconn, ClientFrame::Fail(FailReason::ConfirmMismatch), 3)
        .unwrap();
    assert!(to(&out, sconn, ServerFrame::Burned));
    assert!(to(&out, rconn, ServerFrame::Burned));
    assert!(matches!(
        receiver.on_frame(ServerFrame::Burned).unwrap().as_slice(),
        [ClientEvent::Burned]
    ));
    assert_eq!(place.plates(), 0);
}

#[test]
fn a_third_join_is_busy_and_a_sender_close_is_peer_closed() {
    let mut place = MeetingPlace::new(Config::new(100, 10, 600).unwrap());
    let sconn = place.connect(IP, 0).unwrap();
    let rconn = place.connect(IP, 0).unwrap();
    let tconn = place.connect(IP, 0).unwrap();
    let (mut sender, claim) = Sender::new([1247, 1936], "sluice1abc").unwrap();
    let (mut receiver, join) = Receiver::new(Code::parse("1-orbit-velvet").unwrap());
    place.receive(sconn, claim, 0).unwrap();
    sender.on_frame(ServerFrame::Claimed(1)).unwrap();
    place.receive(rconn, join, 0).unwrap();
    sender.on_frame(ServerFrame::Joined(Role::Sender)).unwrap();
    receiver
        .on_frame(ServerFrame::Joined(Role::Receiver))
        .unwrap();
    assert_eq!(
        place.receive(tconn, ClientFrame::Join(1), 1),
        Err(ErrorCode::Busy)
    );
    let out = place.disconnect(sconn, 2);
    assert!(to(&out, rconn, ServerFrame::PeerClosed));
    assert!(matches!(
        receiver
            .on_frame(ServerFrame::PeerClosed)
            .unwrap()
            .as_slice(),
        [ClientEvent::PeerClosed]
    ));
}

fn to(out: &[crate::meet::Outbound], id: crate::meet::ConnectionId, frame: ServerFrame) -> bool {
    out.iter().any(|one| one.to == id && one.frame == frame)
}

#[test]
fn debug_never_shows_the_words_or_the_ticket() {
    let (sender, _) = Sender::new([1247, 1936], "sluice1secret").unwrap();
    let shown = format!("{sender:?}");
    assert!(!shown.contains("orbit"), "{shown}");
    assert!(!shown.contains("1936"), "{shown}");
    assert!(!shown.contains("sluice1secret"), "{shown}");
    let event = ClientEvent::Ticket(zeroize::Zeroizing::new("sluice1secret".to_string()));
    let shown = format!("{event:?}");
    assert!(!shown.contains("sluice1secret"), "{shown}");
}
