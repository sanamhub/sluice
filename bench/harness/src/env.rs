//! `env.json`: the hardware, kernel and command lines behind a result, as far as they can be
//! read without root.

use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::{
    host::{Host, Logged, Os, quote},
    tools::Tool,
};

const QUICK: Duration = Duration::from_secs(30);

/// One piece of information and the shell script that reads it. A script that fails or prints
/// nothing leaves the field `null`, because the harness reports what it read and nothing else.
type Probe = (&'static str, String);

fn linux(host: &Host) -> Vec<Probe> {
    let mut reads: Vec<Probe> = vec![
        (
            "os",
            "grep -m1 PRETTY_NAME /etc/os-release | cut -d= -f2- | tr -d '\"'".into(),
        ),
        (
            "cpu",
            "grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//'".into(),
        ),
        ("cpu_threads", "nproc".into()),
        (
            "memory_kib",
            "grep -m1 MemTotal /proc/meminfo | tr -s ' ' | cut -d' ' -f2".into(),
        ),
        ("kernel", "uname -srm".into()),
        (
            "file_system",
            format!("df -PT {} | tail -n 1", quote(&host.entry.data_dir)),
        ),
    ];
    if let Some(nic) = &host.entry.interface {
        let nic = quote(nic);
        reads.extend([
            ("nic_mtu", format!("cat /sys/class/net/{nic}/mtu")),
            ("nic_speed_mbit", format!("cat /sys/class/net/{nic}/speed")),
            ("nic_driver", format!("ethtool -i {nic} | grep -E '^(driver|version|firmware-version):' | tr '\\n' ' '")),
        ]);
    }
    reads
}

fn macos(host: &Host) -> Vec<Probe> {
    let mut reads: Vec<Probe> = vec![
        ("os", "sw_vers -productVersion".into()),
        ("cpu", "sysctl -n machdep.cpu.brand_string".into()),
        ("cpu_threads", "sysctl -n hw.ncpu".into()),
        (
            "memory_kib",
            "echo $(( $(sysctl -n hw.memsize) / 1024 ))".into(),
        ),
        ("kernel", "uname -srm".into()),
        (
            "file_system",
            format!("df -P {} | tail -n 1", quote(&host.entry.data_dir)),
        ),
    ];
    if let Some(nic) = &host.entry.interface {
        let nic = quote(nic);
        reads.push((
            "nic_mtu",
            format!("ifconfig {nic} | grep -o 'mtu [0-9]*' | cut -d' ' -f2"),
        ));
    }
    reads
}

async fn read(host: &Host, reads: Vec<Probe>) -> Map<String, Value> {
    let mut details = Map::new();
    for (key, script) in reads {
        let text = match host.run(&["sh", "-c", &script], QUICK).await {
            Ok(out) if out.success() && !out.stdout.trim().is_empty() => {
                Value::String(out.stdout.trim().to_string())
            }
            _ => Value::Null,
        };
        details.insert(key.to_string(), text);
    }
    details
}

/// A Windows host can only be `local`, so the process's own environment answers most questions.
async fn windows(host: &Host) -> Map<String, Value> {
    let mut details = Map::new();
    let var = |name: &str| std::env::var(name).map_or(Value::Null, Value::String);
    details.insert("cpu".to_string(), var("PROCESSOR_IDENTIFIER"));
    let threads = std::thread::available_parallelism().map_or(Value::Null, |n| json!(n.get()));
    details.insert("cpu_threads".to_string(), threads);
    let ver = host.run(&["cmd", "/C", "ver"], QUICK).await;
    let kernel = ver
        .ok()
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty());
    details.insert(
        "kernel".to_string(),
        kernel.map_or(Value::Null, Value::String),
    );
    details
}

/// What is known about one host.
pub async fn host(host: &Host, tools: &[Tool]) -> Value {
    let details = match host.os {
        Os::Linux => read(host, linux(host)).await,
        Os::Macos => read(host, macos(host)).await,
        Os::Windows => windows(host).await,
        Os::Other => Map::new(),
    };
    json!({
        "role": host.role,
        "host": host.entry.host,
        "os_family": host.os,
        "interface": host.entry.interface,
        "probe": host.entry.probe,
        "data_dir": host.entry.data_dir,
        "details": details,
        "tools": tools,
    })
}

/// The document written as `env.json`.
pub fn document(hosts: Vec<Value>, commands: &[Logged]) -> Value {
    json!({
        "harness_version": env!("CARGO_PKG_VERSION"),
        "hosts": hosts,
        "commands": commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_carries_hosts_and_commands() {
        let commands = [Logged {
            host: "sender",
            command: "probe serve --size 1GiB".to_string(),
        }];
        let doc = document(vec![json!({"role": "sender"})], &commands);
        assert_eq!(doc["hosts"][0]["role"], "sender");
        assert_eq!(doc["commands"][0]["command"], "probe serve --size 1GiB");
        assert!(doc["harness_version"].is_string());
    }
}
