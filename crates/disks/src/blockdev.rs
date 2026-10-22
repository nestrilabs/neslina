//! Whole disks as sysfs describes them, and making one into a neslina disk.

use std::fs::{self, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use neslina_common::medium::{self, read_trimmed};

use crate::gpt::{self, Part};

/// Devices that are never disks to install on.
const NOT_DISKS: &[&str] = &["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"];
/// `_IO(0x12, 95)` from linux/fs.h: re-read the partition table.
const BLKRPART: libc::c_ulong = 0x125F;
/// Cleared before writing a new table, at both ends, so no old partition table
/// or filesystem signature survives to confuse anything.
const WIPE_BYTES: u64 = 2 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Nvme,
    Usb,
    Virtio,
    Other,
}

#[derive(Clone, Debug, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Disk {
    /// The kernel's name, `sda` or `nvme0n1`.
    pub name: String,
    #[ts(type = "number")]
    pub size: u64,
    /// Logical block size in bytes.
    #[ts(type = "number")]
    pub block: u64,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub transport: Transport,
    pub removable: bool,
    /// Kernel names of its partitions.
    pub partitions: Vec<String>,
    /// Whether the boot medium is on it. Never offered for wiping.
    pub holds_medium: bool,
}

fn opt(path: PathBuf) -> Option<String> {
    read_trimmed(path).ok().filter(|s| !s.is_empty())
}

pub fn list() -> io::Result<Vec<Disk>> {
    let medium_disks: Vec<String> = medium::find(medium::PARTNAME)?.iter().filter_map(|p| p.disk().ok()).collect();
    let mut disks = Vec::new();
    for entry in fs::read_dir("/sys/block")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if NOT_DISKS.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let sys = entry.path();
        let size = read_trimmed(sys.join("size"))?.parse::<u64>().unwrap_or(0) * 512;
        if size == 0 {
            continue;
        }
        let real = fs::canonicalize(&sys)?.to_string_lossy().into_owned();
        let transport = if name.starts_with("nvme") {
            Transport::Nvme
        } else if real.contains("/usb") {
            Transport::Usb
        } else if real.contains("/virtio") {
            Transport::Virtio
        } else {
            Transport::Other
        };
        let mut partitions: Vec<String> = fs::read_dir(&sys)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("partition").exists())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        partitions.sort();
        disks.push(Disk {
            block: read_trimmed(sys.join("queue/logical_block_size"))?.parse().unwrap_or(512),
            model: opt(sys.join("device/model")),
            serial: opt(sys.join("serial")).or_else(|| opt(sys.join("device/serial"))),
            removable: read_trimmed(sys.join("removable")).is_ok_and(|r| r == "1"),
            holds_medium: medium_disks.contains(&name),
            partitions,
            transport,
            size,
            name,
        });
    }
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(disks)
}

/// The kernel's name for partition `n` of `disk`: `sda1`, `nvme0n1p1`.
pub fn partition_name(disk: &str, n: u32) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) { format!("{disk}p{n}") } else { format!("{disk}{n}") }
}

/// Wipes `disk` and gives it one partition named `partname`, formatted XFS
/// with `label`. Returns the partition's device path.
pub fn make_xfs_disk(disk: &Disk, partname: &str, label: &str) -> io::Result<PathBuf> {
    let dev = Path::new("/dev").join(&disk.name);
    let sectors = disk.size / disk.block;
    let parts = [Part { name: partname.to_owned(), type_guid: gpt::guid(gpt::TYPE_LINUX).unwrap(), size: None }];
    let (writes, _) = gpt::build(sectors, disk.block, gpt::random_guid(), &parts)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{}: {e}", disk.name)))?;

    let f = OpenOptions::new().write(true).open(&dev)?;
    let zeros = vec![0u8; WIPE_BYTES as usize];
    f.write_all_at(&zeros, 0)?;
    f.write_all_at(&zeros, disk.size - WIPE_BYTES)?;
    for w in &writes {
        f.write_all_at(&w.data, w.offset)?;
    }
    f.sync_all()?;
    // SAFETY: BLKRPART takes no argument.
    if unsafe { libc::ioctl(f.as_raw_fd(), BLKRPART) } < 0 {
        return Err(io::Error::last_os_error());
    }
    drop(f);

    let part = partition_name(&disk.name, 1);
    let part_dev = Path::new("/dev").join(&part);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !part_dev.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("{part} did not appear")));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let out = Command::new("/usr/bin/mkfs.xfs").args(["-q", "-f", "-L", label]).arg(&part_dev).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "mkfs.xfs on {part} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(part_dev)
}
