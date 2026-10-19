//! netem on both hosts' interfaces: apply it, read it back, remove it.
//!
//! netem shapes egress. The sender's carries what the data sees (half the delay, the loss, the
//! rate and the queue) and the receiver's carries the other half of the delay for the
//! acknowledgements, so the round trip gets the delay the scenario names.
//!
//! Only `sudo -n tc` runs with privilege. `tc qdisc show` needs none, so the check that the shaping
//! took effect runs as the harness's own user.

mod verify;

use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use self::verify::{rate_bits, verify};
use crate::{host::Host, scenarios::Netem};

const TC_TIMEOUT: Duration = Duration::from_secs(30);

/// What `tc qdisc show` printed on each side after the shaping was applied.
#[derive(Clone, Debug, Serialize)]
pub struct Shown {
    /// The sender's interface.
    pub sender: String,
    /// The receiver's interface.
    pub receiver: String,
}

/// The hosts whose interfaces are shaped right now, so an error or ctrl-c can undo them.
#[derive(Clone, Default)]
pub struct Shaping(Arc<Mutex<Vec<Host>>>);

impl Shaping {
    /// Removes the shaping from every host that has it, the last one shaped first. It tries them
    /// all, and returns the first error. Safe to call when there is none.
    pub async fn undo(&self) -> Result<()> {
        let active: Vec<Host> = std::mem::take(&mut *self.0.lock().expect("shaping"));
        let mut first = None;
        for host in active.iter().rev() {
            if let Err(e) = remove(host).await {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn add(&self, host: Host) {
        self.0.lock().expect("shaping").push(host);
    }
}

fn interface(host: &Host) -> Result<&str> {
    host.entry.interface.as_deref().with_context(|| {
        format!(
            "netem needs `interface` on the {} in the hosts file",
            host.role
        )
    })
}

/// The scenario's netem split between the two egress directions: the sender's, which the data
/// crosses, and the receiver's, which the acknowledgements cross.
pub fn split(netem: &Netem) -> (Netem, Netem) {
    let data = Netem {
        delay_ms: netem.delay_ms.map(|d| d / 2.0),
        jitter_ms: None,
        loss_percent: netem.loss_percent,
        rate: netem.rate.clone(),
        limit_packets: netem.limit_packets,
    };
    let acks = Netem {
        delay_ms: netem.delay_ms.map(|d| d / 2.0),
        jitter_ms: netem.jitter_ms.map(|j| j / 2.0),
        loss_percent: None,
        rate: None,
        limit_packets: None,
    };
    (data, acks)
}

/// The `tc` arguments after `qdisc replace dev <interface> root netem`.
pub fn parameters(netem: &Netem) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(delay) = netem.delay_ms {
        args.extend(["delay".to_string(), format!("{delay}ms")]);
        if let Some(jitter) = netem.jitter_ms {
            args.push(format!("{jitter}ms"));
        }
    }
    if let Some(loss) = netem.loss_percent {
        args.extend(["loss".to_string(), format!("{loss}%")]);
    }
    if let Some(rate) = &netem.rate {
        args.extend(["rate".to_string(), rate.clone()]);
    }
    if let Some(limit) = netem.limit_packets {
        args.extend(["limit".to_string(), limit.to_string()]);
    }
    args
}

/// Shapes both interfaces and returns what `tc qdisc show` printed on each. If either side
/// fails, the shaping that was applied is removed before the error is returned.
pub async fn apply(
    shaping: &Shaping,
    sender: &Host,
    receiver: &Host,
    netem: &Netem,
) -> Result<Shown> {
    let (data, acks) = split(netem);
    let (sender, receiver) = both(
        apply_side(shaping, sender, &data),
        apply_side(shaping, receiver, &acks),
        shaping.undo(),
    )
    .await?;
    Ok(Shown { sender, receiver })
}

/// Runs `first`, then `second`. If either fails, `undo` runs before the error is returned, so a
/// side that did apply is not left shaped. `undo` is not run when both succeed.
async fn both<A, B>(
    first: impl Future<Output = Result<A>>,
    second: impl Future<Output = Result<B>>,
    undo: impl Future<Output = Result<()>>,
) -> Result<(A, B)> {
    let applied: Result<(A, B)> = async {
        let a = first.await?;
        let b = second.await?;
        Ok((a, b))
    }
    .await;
    match applied {
        Ok(pair) => Ok(pair),
        Err(error) => match undo.await {
            Ok(()) => Err(error),
            Err(undone) => {
                Err(error.context(format!("removing the shaping also failed: {undone:#}")))
            }
        },
    }
}

async fn apply_side(shaping: &Shaping, host: &Host, netem: &Netem) -> Result<String> {
    let interface = interface(host)?;
    if let Some(rate) = &netem.rate {
        rate_bits(rate).with_context(|| format!("rate {rate:?}"))?;
    }
    let before = show(host, interface).await?;
    ensure!(
        !before.contains("netem"),
        "{interface} on the {} already has a netem qdisc: {}. Remove it with `sudo tc qdisc del dev {interface} root`.",
        host.role,
        before.trim()
    );
    let mut argv = vec![
        "sudo", "-n", "tc", "qdisc", "replace", "dev", interface, "root", "netem",
    ];
    let params = parameters(netem);
    argv.extend(params.iter().map(String::as_str));
    // Registered first: if the check below fails, the shaping is already undoable.
    shaping.add(host.clone());
    let out = host.run(&argv, TC_TIMEOUT).await?;
    if !out.success() {
        let hint = if out.stderr.contains("password") {
            " The user needs passwordless `sudo -n tc`."
        } else {
            ""
        };
        bail!(
            "tc qdisc replace failed on the {}: {}.{hint}",
            host.role,
            out.stderr.trim()
        );
    }
    let shown = show(host, interface).await?;
    verify(&shown, netem).with_context(|| {
        format!(
            "tc qdisc show on the {} printed: {}",
            host.role,
            shown.trim()
        )
    })?;
    Ok(shown)
}

/// Removes the netem qdisc from a host's interface, if it has one.
pub async fn remove(host: &Host) -> Result<()> {
    let interface = interface(host)?;
    // A side whose apply failed before tc changed anything has nothing to delete, and `tc qdisc
    // del` fails on that and would hide the error that mattered.
    if !show(host, interface).await?.contains("netem") {
        return Ok(());
    }
    let argv = ["sudo", "-n", "tc", "qdisc", "del", "dev", interface, "root"];
    let out = host.run(&argv, TC_TIMEOUT).await?;
    ensure!(
        out.success(),
        "could not remove netem from {interface} on the {}: {}. Run `sudo tc qdisc del dev {interface} root`.",
        host.role,
        out.stderr.trim()
    );
    Ok(())
}

async fn show(host: &Host, interface: &str) -> Result<String> {
    let out = host
        .run(&["tc", "qdisc", "show", "dev", interface], TC_TIMEOUT)
        .await?;
    ensure!(out.success(), "tc qdisc show failed: {}", out.stderr.trim());
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3() -> Netem {
        Netem {
            delay_ms: Some(80.0),
            jitter_ms: None,
            loss_percent: Some(0.1),
            rate: Some("1gbit".to_string()),
            limit_packets: Some(20000),
        }
    }

    #[test]
    fn the_arguments_follow_the_scenario() {
        assert_eq!(
            parameters(&s3()).join(" "),
            "delay 80ms loss 0.1% rate 1gbit limit 20000"
        );
        let jittery = Netem {
            delay_ms: Some(40.0),
            jitter_ms: Some(10.0),
            ..s3()
        };
        assert!(
            parameters(&jittery)
                .join(" ")
                .starts_with("delay 40ms 10ms loss")
        );
    }

    #[test]
    fn the_data_side_takes_half_the_delay_and_all_the_loss_and_rate() {
        let (data, _) = split(&s3());
        assert_eq!(
            parameters(&data).join(" "),
            "delay 40ms loss 0.1% rate 1gbit limit 20000"
        );
    }

    #[test]
    fn the_ack_side_takes_the_other_half_of_the_delay_and_of_the_jitter() {
        let jittery = Netem {
            jitter_ms: Some(10.0),
            ..s3()
        };
        let (data, acks) = split(&jittery);
        assert_eq!(parameters(&acks).join(" "), "delay 40ms 5ms");
        // The data side has no jitter, so the round trip's variation is not counted twice.
        assert_eq!(data.jitter_ms, None);
    }

    #[test]
    fn the_two_halves_add_up_to_the_round_trip() {
        let (data, acks) = split(&s3());
        assert_eq!(data.delay_ms.unwrap() + acks.delay_ms.unwrap(), 80.0);
    }

    #[test]
    fn a_link_with_only_a_rate_has_nothing_to_delay_on_the_ack_side() {
        let rate_only = Netem {
            delay_ms: None,
            jitter_ms: None,
            loss_percent: None,
            rate: Some("1gbit".to_string()),
            limit_packets: None,
        };
        let (data, acks) = split(&rate_only);
        assert_eq!(parameters(&data).join(" "), "rate 1gbit");
        assert!(parameters(&acks).is_empty());
    }

    fn log(entries: &Mutex<Vec<&'static str>>, entry: &'static str) {
        entries.lock().unwrap().push(entry);
    }

    #[tokio::test]
    async fn both_sides_apply_in_order_and_nothing_is_undone_on_success() {
        let entries = Mutex::new(Vec::new());
        let got = both(
            async {
                log(&entries, "sender");
                Ok(1)
            },
            async {
                log(&entries, "receiver");
                Ok(2)
            },
            async {
                log(&entries, "undo");
                Ok(())
            },
        )
        .await
        .expect("both apply");
        assert_eq!(got, (1, 2));
        assert_eq!(*entries.lock().unwrap(), ["sender", "receiver"]);
    }

    #[tokio::test]
    async fn when_the_second_side_fails_the_first_is_undone_before_the_error_returns() {
        let entries = Mutex::new(Vec::new());
        let failed = both(
            async {
                log(&entries, "sender");
                Ok(1)
            },
            async {
                log(&entries, "receiver");
                Err::<i32, _>(anyhow::anyhow!("tc qdisc replace failed"))
            },
            async {
                log(&entries, "undo");
                Ok(())
            },
        )
        .await
        .expect_err("the receiver fails");
        assert_eq!(*entries.lock().unwrap(), ["sender", "receiver", "undo"]);
        assert!(format!("{failed:#}").contains("tc qdisc replace failed"));
    }

    #[tokio::test]
    async fn when_the_first_side_fails_the_second_is_never_tried() {
        let entries = Mutex::new(Vec::new());
        let failed = both(
            async {
                log(&entries, "sender");
                Err::<i32, _>(anyhow::anyhow!("no sudo"))
            },
            async {
                log(&entries, "receiver");
                Ok(2)
            },
            async {
                log(&entries, "undo");
                Ok(())
            },
        )
        .await
        .expect_err("the sender fails");
        assert_eq!(*entries.lock().unwrap(), ["sender", "undo"]);
        assert!(format!("{failed:#}").contains("no sudo"));
    }

    #[tokio::test]
    async fn a_failed_undo_adds_to_the_error_and_does_not_replace_it() {
        let failed = both(
            async { Ok(1) },
            async { Err::<i32, _>(anyhow::anyhow!("replace failed")) },
            async { Err(anyhow::anyhow!("could not remove")) },
        )
        .await
        .expect_err("fails");
        let text = format!("{failed:#}");
        assert!(text.contains("replace failed"), "{text}");
        assert!(text.contains("could not remove"), "{text}");
    }
}
