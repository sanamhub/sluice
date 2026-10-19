//! Reading `tc qdisc show` back: did the qdisc get what was asked for?

use anyhow::{Context, Result, ensure};

use crate::scenarios::Netem;

/// Fails unless `shown` is a root netem qdisc with every parameter `netem` asked for.
pub fn verify(shown: &str, netem: &Netem) -> Result<()> {
    let tokens: Vec<&str> = shown.split_whitespace().collect();
    ensure!(
        tokens.contains(&"netem") && tokens.contains(&"root"),
        "no root netem qdisc"
    );
    let after = |key: &str, skip: usize| {
        let at = tokens.iter().position(|t| *t == key)?;
        tokens.get(at + 1 + skip).copied()
    };
    if let Some(want) = netem.delay_ms {
        let got = after("delay", 0).and_then(micros);
        ensure!(
            close(got, want * 1000.0, 1.0),
            "delay is {got:?} us, wanted {want} ms"
        );
    }
    if let Some(want) = netem.jitter_ms {
        let got = after("delay", 1).and_then(micros);
        ensure!(
            close(got, want * 1000.0, 1.0),
            "jitter is {got:?} us, wanted {want} ms"
        );
    }
    if let Some(want) = netem.loss_percent {
        let got = after("loss", 0).and_then(|t| t.strip_suffix('%')?.parse().ok());
        ensure!(close(got, want, 1e-3), "loss is {got:?} %, wanted {want} %");
    }
    if let Some(want) = &netem.rate {
        let got = after("rate", 0).and_then(rate_bits);
        ensure!(
            close(got, rate_bits(want).context("rate")?, 1.0),
            "rate is {got:?} bit/s, wanted {want}"
        );
    }
    if let Some(want) = netem.limit_packets {
        let got: Option<u32> = after("limit", 0).and_then(|t| t.parse().ok());
        ensure!(got == Some(want), "limit is {got:?}, wanted {want}");
    }
    Ok(())
}

fn close(got: Option<f64>, want: f64, tolerance: f64) -> bool {
    got.is_some_and(|got| (got - want).abs() <= tolerance)
}

/// A `tc` time such as `80ms`, `500us` or `1.5s`, in microseconds.
fn micros(token: &str) -> Option<f64> {
    let split = token.find(|c: char| c.is_ascii_alphabetic())?;
    let value: f64 = token[..split].parse().ok()?;
    match &token[split..] {
        "us" => Some(value),
        "ms" => Some(value * 1e3),
        "s" => Some(value * 1e6),
        _ => None,
    }
}

/// A `tc` rate such as `1gbit` or `100Mbit`, in bits per second. `tc` prints `1Gbit` for what
/// was asked as `1gbit`, so case does not matter.
pub fn rate_bits(token: &str) -> Option<f64> {
    let token = token.to_ascii_lowercase();
    let split = token.find(|c: char| c.is_ascii_alphabetic())?;
    let value: f64 = token[..split].parse().ok()?;
    let unit = match &token[split..] {
        "bit" => 1.0,
        "kbit" => 1e3,
        "mbit" => 1e6,
        "gbit" => 1e9,
        "tbit" => 1e12,
        _ => return None,
    };
    Some(value * unit)
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

    const SHOWN: &str =
        "qdisc netem 8001: root refcnt 2 limit 20000 delay 80ms loss 0.1% rate 1Gbit\n";

    #[test]
    fn what_tc_prints_matches_what_was_asked() {
        verify(SHOWN, &s3()).expect("matches");
        let jittery = Netem {
            delay_ms: Some(40.0),
            jitter_ms: Some(10.0),
            loss_percent: None,
            rate: Some("10gbit".to_string()),
            limit_packets: None,
        };
        let shown = "qdisc netem 8002: root refcnt 2 limit 1000 delay 40ms  10ms rate 10Gbit";
        verify(shown, &jittery).expect("matches with jitter");
    }

    #[test]
    fn a_mismatch_is_reported() {
        let wrong_delay = SHOWN.replace("80ms", "8ms");
        assert!(verify(&wrong_delay, &s3()).is_err());
        let wrong_rate = SHOWN.replace("1Gbit", "100Mbit");
        assert!(verify(&wrong_rate, &s3()).is_err());
        let no_loss = SHOWN.replace(" loss 0.1%", "");
        assert!(verify(&no_loss, &s3()).is_err());
        let not_root = SHOWN.replace("root", "parent 1:1");
        assert!(verify(&not_root, &s3()).is_err());
        assert!(verify("qdisc mq 0: root", &s3()).is_err());
    }

    #[test]
    fn units_convert() {
        assert_eq!(micros("80ms"), Some(80_000.0));
        assert_eq!(micros("500us"), Some(500.0));
        assert_eq!(micros("1.5s"), Some(1_500_000.0));
        assert_eq!(micros("80"), None);
        assert_eq!(rate_bits("1Gbit"), Some(1e9));
        assert_eq!(rate_bits("100mbit"), Some(1e8));
        assert_eq!(rate_bits("1000Mbit"), rate_bits("1gbit"));
        assert_eq!(rate_bits("fast"), None);
    }
}
