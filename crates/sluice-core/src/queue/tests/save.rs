//! The queue in `db/queue`: what a restart keeps, what it changes, and what a load refuses.

use std::fs;
use std::path::PathBuf;

use proptest::prelude::*;
use tempfile::TempDir;

use super::super::save::{decode, encode, file_in};
use super::super::{
    Action, HISTORY, JobId, MAX_JOBS, MAX_PATH_UNITS, Priority, Queue, State, Work,
};
use super::props::{op, run};
use super::{add, queue_of, starts};
use crate::ticket::{Body, Format, Ticket};
use crate::{Error, ErrorKind, Home};

fn home() -> (TempDir, Home) {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = Home::open(dir.path()).expect("a home");
    (dir, home)
}

/// What a restart makes of `queue`: every job that ran or waited to retry is waiting.
fn restarted(queue: &Queue) -> Queue {
    let mut after = queue.clone();
    for job in after.jobs.values_mut() {
        if matches!(job.state, State::Active | State::Retrying { .. }) {
            job.state = State::Waiting;
        }
    }
    after
}

#[test]
fn no_file_is_an_empty_queue_with_nothing_to_do() {
    let (_dir, home) = home();
    let (queue, actions) = Queue::load(&home).expect("load");
    assert_eq!(queue, Queue::new());
    assert!(actions.is_empty());
}

#[test]
fn a_restart_puts_running_and_retrying_jobs_in_line_and_keeps_paused_ones_paused() {
    let (_dir, home) = home();
    let mut queue = queue_of(5);
    queue
        .failed(JobId(1), ErrorKind::Network, 0)
        .expect("failed");
    queue.pause(JobId(5)).expect("pause");
    queue.set_slots(2).expect("slots");
    queue.finished(JobId(2)).expect("finished");
    queue.save(&home).expect("save");

    let (loaded, actions) = Queue::load(&home).expect("load");
    // Job 1 waited to retry, and is first in line again. Job 3 ran, and is next.
    assert_eq!(
        actions,
        [
            Action::Start(JobId(1)),
            Action::Start(JobId(3)),
            Action::Save
        ]
    );
    assert_eq!(loaded.job(JobId(1)).expect("job").network_failures(), 1);
    assert_eq!(loaded.job(JobId(4)).expect("job").state(), State::Waiting);
    assert_eq!(loaded.job(JobId(5)).expect("job").state(), State::Paused);
    assert_eq!(loaded.slots(), 2);
    assert_eq!(loaded.history()[0].id(), JobId(2));
}

#[test]
fn ids_go_on_counting_after_a_restart() {
    let (_dir, home) = home();
    let mut queue = queue_of(3);
    queue.cancel(JobId(3)).expect("cancel");
    queue.save(&home).expect("save");
    let (mut loaded, _) = Queue::load(&home).expect("load");
    assert_eq!(add(&mut loaded, "next", Priority::Normal), JobId(4));
}

#[test]
fn a_loaded_queue_starts_its_jobs_in_the_order_it_would_have() {
    let mut queue = Queue::new();
    queue.set_slots(1).expect("slots");
    let first = add(&mut queue, "first", Priority::High);
    for (at, priority) in [Priority::Low, Priority::Normal, Priority::High]
        .iter()
        .cycle()
        .take(12)
        .enumerate()
    {
        add(&mut queue, &format!("f{at}"), *priority);
    }
    queue.pause(JobId(6)).expect("pause");
    let order = |mut queue: Queue| {
        let mut order = vec![first.0];
        let mut current = first;
        while let Some(&id) = starts(&queue.finished(current).expect("finished")).first() {
            order.push(id);
            current = JobId(id);
        }
        order
    };
    let (_dir, home) = home();
    queue.save(&home).expect("save");
    let (loaded, actions) = Queue::load(&home).expect("load");
    assert_eq!(
        starts(&actions),
        [first.0],
        "the job that ran is first in its line again"
    );
    assert_eq!(order(loaded), order(queue));
}

#[test]
fn the_saved_queue_holds_receive_jobs_and_their_tickets() {
    let (_dir, home) = home();
    let ticket = Ticket::new([1; 32], vec![], None, Some(5), live_body()).expect("ticket");
    let mut queue = Queue::new();
    let work = Work::Receive {
        ticket: ticket.clone(),
        path: PathBuf::from("into"),
    };
    let (id, _) = queue.add(work.clone(), Priority::Low).expect("add");
    queue.save(&home).expect("save");
    let (loaded, _) = Queue::load(&home).expect("load");
    assert_eq!(loaded.job(id).expect("job").work(), &work);
}

fn live_body() -> Body {
    Body::Live {
        stream_id: [9; 16],
        sender_key: [0x55; 32],
    }
}

#[cfg(unix)]
#[test]
fn the_file_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, home) = home();
    queue_of(1).save(&home).expect("save");
    let mode = fs::metadata(file_in(dir.path()))
        .expect("file")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// Writes `bytes` as the queue file, loads it, and checks the load fails with a storage error that
/// names the file and says `why`, and that the file is as it was.
fn refused(bytes: &[u8], why: &str) {
    let (dir, home) = home();
    let path = file_in(dir.path());
    fs::write(&path, bytes).expect("write");
    let Err(Error::Storage(message)) = Queue::load(&home) else {
        panic!("expected a storage error for: {why}");
    };
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.contains(why), "{why} not in: {message}");
    assert_eq!(
        fs::read(&path).expect("still there"),
        bytes,
        "the file changed"
    );
}

#[test]
fn a_file_over_16_mib_is_refused() {
    let mut bytes = encode(&queue_of(1)).expect("encode");
    bytes.resize((16 << 20) + 1, 0);
    refused(&bytes, "over 16 MiB");
}

#[test]
fn an_unknown_version_is_refused() {
    let mut bytes = encode(&queue_of(1)).expect("encode");
    bytes[0] = 2;
    refused(&bytes, "version 2");
    refused(&[], "empty");
}

#[test]
fn bytes_that_do_not_decode_are_refused() {
    let good = encode(&queue_of(2)).expect("encode");
    refused(&good[..good.len() - 3], "does not decode");
    refused(&[1, 0xff, 0xff, 0xff], "does not decode");
    let mut long = good.clone();
    long.push(0);
    refused(&long, "bytes after the queue");
}

/// A queue changed by `change` and saved, which a load must refuse with `why`.
fn refused_queue(why: &str, change: impl FnOnce(&mut Queue)) {
    let mut queue = queue_of(3);
    queue.finished(JobId(3)).expect("finished");
    change(&mut queue);
    refused(&encode(&queue).expect("encode"), why);
}

#[test]
fn a_queue_this_one_could_not_have_saved_is_refused() {
    refused_queue("slot count is 0", |q| q.slots = 0);
    refused_queue("next job id is 0", |q| q.next_id = 0);
    refused_queue("out of order or used twice", |q| q.next_id = 2);
    refused_queue("out of order or used twice", |q| {
        let copy = q.jobs[&JobId(1)].clone();
        q.history.push_back(copy);
    });
    refused_queue("a state the queue never leaves", |q| {
        q.jobs.get_mut(&JobId(1)).expect("job").state = State::Done;
    });
    refused_queue("a state the queue never leaves", |q| {
        q.history[0].state = State::Waiting;
    });
    refused_queue("a state the queue never leaves", |q| {
        q.jobs.get_mut(&JobId(2)).expect("job").failures = 10;
    });
    refused_queue("a path of 0 units", |q| {
        q.jobs.get_mut(&JobId(1)).expect("job").work = Work::Send {
            path: PathBuf::new(),
        };
    });
}

#[test]
fn a_ticket_that_does_not_decode_is_refused_without_quoting_it() {
    let mut queue = Queue::new();
    let ticket = Ticket::new([1; 32], vec![], None, None, live_body()).expect("ticket");
    let text = ticket.encode();
    let work = Work::Receive {
        ticket,
        path: PathBuf::from("into"),
    };
    queue.add(work, Priority::Normal).expect("add");
    let mut bytes = encode(&queue).expect("encode");
    // The ticket's text is in the file as it is. Make one of its characters an `I`, which the
    // alphabet leaves out.
    let at = bytes
        .windows(text.len())
        .position(|w| w == text.as_bytes())
        .expect("the ticket in the file");
    bytes[at + 20] = b'I';
    refused(&bytes, "bad ticket");
    let message = Queue::load(&home_with(&bytes).1).unwrap_err().to_string();
    assert!(!message.contains(&text[10..30]), "{message}");
}

fn home_with(bytes: &[u8]) -> (TempDir, Home) {
    let (dir, home) = home();
    fs::write(file_in(dir.path()), bytes).expect("write");
    (dir, home)
}

/// The longest ticket text there is, near 4096 characters, made with many addresses.
fn longest_ticket() -> Ticket {
    let mut addrs = Vec::new();
    loop {
        addrs.push(
            format!("[2001:db8::{:x}]:65535", addrs.len() + 1)
                .parse()
                .expect("addr"),
        );
        let body = Body::Static {
            root: [0xff; 32],
            size: u64::MAX,
            format: Format::HashSequence,
        };
        let relay = Some("https://relay.example/".to_string());
        match Ticket::new([1; 32], addrs.clone(), relay, Some(u64::MAX), body) {
            Ok(ticket) => {
                if ticket.encode().len() > 4000 {
                    return ticket;
                }
            }
            Err(_) => panic!("a ticket of {} addresses is too long", addrs.len()),
        }
    }
}

#[test]
fn the_fullest_queue_there_can_be_fits_under_the_limit_of_a_load() {
    // The most bytes a path can take in the file: 4096 on Unix, where this letter is 3 bytes, and
    // 4096 UTF-16 units of 2 bytes each on Windows, where it is one unit.
    let per_letter = if cfg!(windows) { 1 } else { 3 };
    let path = PathBuf::from("\u{4e2d}".repeat(MAX_PATH_UNITS / per_letter));
    let ticket = longest_ticket();
    let mut queue = Queue::new();
    for _ in 0..MAX_JOBS + HISTORY {
        let work = Work::Receive {
            ticket: ticket.clone(),
            path: path.clone(),
        };
        let (id, _) = queue.add(work, Priority::Low).expect("add");
        if queue.history.len() < HISTORY {
            queue.cancel(id).expect("cancel");
        }
    }
    let bytes = encode(&queue).expect("encode");
    assert!(bytes.len() < 16 << 20, "{} bytes", bytes.len());
    assert_eq!(decode(&bytes).expect("decode"), restarted(&queue));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn any_queue_saved_and_loaded_is_the_same_apart_from_a_restart(
        ops in prop::collection::vec(op(), 1..200)
    ) {
        let queue = run(&ops);
        let bytes = encode(&queue).expect("encode");
        prop_assert_eq!(decode(&bytes).expect("decode"), restarted(&queue));
    }
}
