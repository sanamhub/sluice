use std::net::{IpAddr, Ipv4Addr};

use super::*;
use crate::frame::FailReason;

const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
const OTHER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));

fn place(rate: u32, plates: usize, ttl: u64) -> MeetingPlace {
    MeetingPlace::new(Config::new(rate, plates, ttl).unwrap())
}

fn to(out: &[Outbound], id: ConnectionId, frame: ServerFrame) -> bool {
    out.iter().any(|one| one.to == id && one.frame == frame)
}

#[test]
fn a_claim_and_join_meet_and_the_sender_close_releases_the_plate() {
    let mut place = place(100, 10, 600);
    let sender = place.connect(IP, 0).unwrap();
    let receiver = place.connect(IP, 0).unwrap();
    let out = place.receive(sender, ClientFrame::Claim, 0).unwrap();
    assert!(to(&out, sender, ServerFrame::Claimed(1)));
    assert_eq!(place.plates(), 1);

    let out = place.receive(receiver, ClientFrame::Join(1), 0).unwrap();
    assert!(to(&out, sender, ServerFrame::Joined(Role::Sender)));
    assert!(to(&out, receiver, ServerFrame::Joined(Role::Receiver)));

    let out = place
        .receive(sender, ClientFrame::Spake(vec![7; 33]), 1)
        .unwrap();
    assert!(to(&out, receiver, ServerFrame::Spake(vec![7; 33])));
    let out = place
        .receive(receiver, ClientFrame::Confirm([8; 32]), 1)
        .unwrap();
    assert!(to(&out, sender, ServerFrame::Confirm([8; 32])));

    let out = place.disconnect(sender, 2);
    assert!(to(&out, receiver, ServerFrame::PeerClosed));
    assert_eq!(place.plates(), 0);
}

#[test]
fn a_third_connection_to_a_held_plate_is_busy() {
    let mut place = place(100, 10, 600);
    let sender = place.connect(IP, 0).unwrap();
    let first = place.connect(IP, 0).unwrap();
    let third = place.connect(IP, 0).unwrap();
    place.receive(sender, ClientFrame::Claim, 0).unwrap();
    place.receive(first, ClientFrame::Join(1), 0).unwrap();
    assert_eq!(
        place.receive(third, ClientFrame::Join(1), 0),
        Err(ErrorCode::Busy)
    );
}

#[test]
fn a_failed_confirmation_burns_both_sides() {
    let mut place = place(100, 10, 600);
    let sender = place.connect(IP, 0).unwrap();
    let receiver = place.connect(IP, 0).unwrap();
    place.receive(sender, ClientFrame::Claim, 0).unwrap();
    place.receive(receiver, ClientFrame::Join(1), 0).unwrap();
    let out = place
        .receive(receiver, ClientFrame::Fail(FailReason::ConfirmMismatch), 1)
        .unwrap();
    assert!(to(&out, sender, ServerFrame::Burned));
    assert!(to(&out, receiver, ServerFrame::Burned));
    assert_eq!(place.plates(), 0);
    let third = place.connect(OTHER, 2).unwrap();
    assert_eq!(
        place.receive(third, ClientFrame::Join(1), 2),
        Err(ErrorCode::NoCode)
    );
}

#[test]
fn a_receiver_that_closes_before_done_burns_the_plate() {
    let mut place = place(100, 10, 600);
    let sender = place.connect(IP, 0).unwrap();
    let receiver = place.connect(IP, 0).unwrap();
    place.receive(sender, ClientFrame::Claim, 0).unwrap();
    place.receive(receiver, ClientFrame::Join(1), 0).unwrap();
    let out = place.disconnect(receiver, 1);
    assert!(to(&out, sender, ServerFrame::Burned));
    assert_eq!(place.plates(), 0);
}

#[test]
fn done_releases_the_plate_and_tells_the_sender() {
    let mut place = place(100, 10, 600);
    let sender = place.connect(IP, 0).unwrap();
    let receiver = place.connect(IP, 0).unwrap();
    place.receive(sender, ClientFrame::Claim, 0).unwrap();
    place.receive(receiver, ClientFrame::Join(1), 0).unwrap();
    let out = place.receive(receiver, ClientFrame::Done, 2).unwrap();
    assert!(to(&out, sender, ServerFrame::Done));
    assert_eq!(place.plates(), 0);
    assert!(place.disconnect(sender, 3).is_empty());
}

#[test]
fn a_plate_past_its_ttl_expires_for_both_sides() {
    let mut place = place(100, 10, 60);
    let sender = place.connect(IP, 0).unwrap();
    let receiver = place.connect(IP, 0).unwrap();
    place.receive(sender, ClientFrame::Claim, 0).unwrap();
    place.receive(receiver, ClientFrame::Join(1), 0).unwrap();
    assert!(place.expire(59_999).is_empty());
    let out = place.expire(60_000);
    assert!(to(&out, sender, ServerFrame::Error(ErrorCode::Expired)));
    assert!(to(&out, receiver, ServerFrame::Error(ErrorCode::Expired)));
    assert_eq!(place.plates(), 0);
}

#[test]
fn a_join_after_the_ttl_gets_expired_and_the_holders_are_told() {
    let mut place = place(100, 10, 60);
    let sender = place.connect(IP, 0).unwrap();
    let receiver = place.connect(IP, 0).unwrap();
    place.receive(sender, ClientFrame::Claim, 0).unwrap();
    assert_eq!(
        place.receive(receiver, ClientFrame::Join(1), 60_000),
        Err(ErrorCode::Expired)
    );
    assert_eq!(place.plates(), 0);
}

#[test]
fn the_table_stops_at_max_plates() {
    let mut place = place(100, 1, 600);
    let first = place.connect(IP, 0).unwrap();
    let second = place.connect(OTHER, 0).unwrap();
    place.receive(first, ClientFrame::Claim, 0).unwrap();
    assert_eq!(
        place.receive(second, ClientFrame::Claim, 1),
        Err(ErrorCode::Full)
    );
}

#[test]
fn the_per_ip_rate_is_spent_and_refills() {
    let mut place = place(1, 10, 600);
    let first = place.connect(IP, 0).unwrap();
    let second = place.connect(IP, 0).unwrap();
    place.receive(first, ClientFrame::Claim, 0).unwrap();
    assert_eq!(
        place.receive(second, ClientFrame::Claim, 1),
        Err(ErrorCode::RateLimited)
    );
    assert!(place.receive(second, ClientFrame::Claim, 60_000).is_ok());
}

#[test]
fn the_per_ip_connection_cap_is_counted_and_released() {
    let mut place = MeetingPlace::new(
        Config::new(10, 10, 600)
            .unwrap()
            .with_connections(1)
            .unwrap(),
    );
    let first = place.connect(IP, 0).unwrap();
    assert_eq!(place.connect(IP, 0), Err(ErrorCode::RateLimited));
    assert_eq!(place.connections_for(IP), 1);
    place.disconnect(first, 1);
    assert_eq!(place.connections_for(IP), 0);
    assert!(place.connect(IP, 2).is_ok());
}

#[test]
fn only_a_sender_may_send_sealed_and_only_a_receiver_may_send_done() {
    let mut place = place(100, 10, 600);
    let sender = place.connect(IP, 0).unwrap();
    let receiver = place.connect(IP, 0).unwrap();
    place.receive(sender, ClientFrame::Claim, 0).unwrap();
    place.receive(receiver, ClientFrame::Join(1), 0).unwrap();
    assert_eq!(
        place.receive(receiver, ClientFrame::Sealed(vec![0; 28]), 1),
        Err(ErrorCode::BadFrame)
    );
    assert_eq!(
        place.receive(sender, ClientFrame::Done, 1),
        Err(ErrorCode::BadFrame)
    );
}

#[test]
fn nameplates_are_reused_after_a_plate_is_released_and_stay_in_range() {
    let mut place = place(100, 9999, 600);
    let mut seen = std::collections::BTreeSet::new();
    for now in 0..10 {
        let id = place.connect(IP, now).unwrap();
        let out = place.receive(id, ClientFrame::Claim, now).unwrap();
        let plate = match out.first().map(|one| &one.frame) {
            Some(ServerFrame::Claimed(plate)) => *plate,
            other => panic!("{other:?}"),
        };
        assert!((1..=9999).contains(&plate));
        assert!(seen.insert(plate), "a held plate was handed out twice");
        place.disconnect(id, now);
    }
}
