//! What the setup UI shows about the machine.

use std::fs;

use neslina_common::medium;
use neslina_common::sys::is_mountpoint;
use neslina_disks::blockdev::{self, Disk};
use serde::Serialize;

#[derive(Serialize)]
pub struct Host {
    pub hostname: String,
    pub slot: String,
    pub version: String,
    pub cpu: String,
    pub threads: usize,
    pub memory_bytes: u64,
    pub disks: Vec<Disk>,
    /// Whether the state disk is set up and mounted.
    pub state_disk: bool,
}

fn first_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

pub fn read() -> Host {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let os = fs::read_to_string("/etc/os-release").unwrap_or_default();
    Host {
        hostname: fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim().to_owned(),
        slot: medium::current_slot().unwrap_or_default(),
        version: os.lines().find_map(|l| l.strip_prefix("VERSION_ID=")).unwrap_or_default().to_owned(),
        cpu: first_value(&cpuinfo, "model name").unwrap_or_default().to_owned(),
        threads: std::thread::available_parallelism().map_or(0, |n| n.get()),
        memory_bytes: first_value(&meminfo, "MemTotal")
            .and_then(|v| v.trim_end_matches("kB").trim().parse::<u64>().ok())
            .map_or(0, |kb| kb * 1024),
        disks: blockdev::list().unwrap_or_default(),
        state_disk: is_mountpoint("/var"),
    }
}
