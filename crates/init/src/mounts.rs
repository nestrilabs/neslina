//! The filesystems a read-only root needs before anything runs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use neslina_common::sys::{is_mountpoint, mount, umount};
use neslina_common::warn;

const NOSUID_NODEV_NOEXEC: libc::c_ulong = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;

/// Mounted below the root's own directories. Stage 1 has already moved /dev,
/// /proc and /sys in; they are only mounted here if init runs without it.
const MOUNTS: &[(&str, &str, &str, libc::c_ulong, Option<&str>)] = &[
    ("devtmpfs", "/dev", "devtmpfs", libc::MS_NOSUID, None),
    ("proc", "/proc", "proc", NOSUID_NODEV_NOEXEC, None),
    ("sysfs", "/sys", "sysfs", NOSUID_NODEV_NOEXEC, None),
    ("tmpfs", "/run", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, Some("mode=0755")),
    ("tmpfs", "/tmp", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, Some("mode=1777")),
    ("devpts", "/dev/pts", "devpts", libc::MS_NOSUID | libc::MS_NOEXEC, Some("gid=5,mode=0620,ptmxmode=0666")),
    ("tmpfs", "/dev/shm", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, Some("mode=1777")),
    (
        "cgroup2",
        "/sys/fs/cgroup",
        "cgroup2",
        NOSUID_NODEV_NOEXEC,
        // Hugetlb-backed guest RAM counts against memory.max like any other.
        Some("nsdelegate,memory_recursiveprot,memory_hugetlb_accounting"),
    ),
];

const EFIVARS: &str = "/sys/firmware/efi/efivars";

/// Owned by init; services get their own cgroup under it.
pub const SYSTEM_CGROUP: &str = "/sys/fs/cgroup/system";

pub fn setup() -> Result<()> {
    for &(source, target, fstype, flags, data) in MOUNTS {
        if is_mountpoint(target) {
            continue;
        }
        fs::create_dir_all(target).ok();
        mount(source, target, fstype, flags, data).with_context(|| format!("mounting {target}"))?;
    }
    // Slots are UEFI boot entries; the boot service reads and writes them here.
    if Path::new(EFIVARS).parent().is_some_and(Path::exists) && !is_mountpoint(EFIVARS) {
        mount("efivarfs", EFIVARS, "efivarfs", NOSUID_NODEV_NOEXEC, None).context("mounting efivarfs")?;
    }
    fs::create_dir_all("/run/neslina").context("creating /run/neslina")?;
    cgroups()
}

/// Turns on every controller the kernel has, at the root and for the system
/// subtree, so services (and later boxes) can be limited.
fn cgroups() -> Result<()> {
    let available = fs::read_to_string("/sys/fs/cgroup/cgroup.controllers").context("reading cgroup.controllers")?;
    let enable: String = available.split_whitespace().map(|c| format!("+{c} ")).collect();
    fs::write("/sys/fs/cgroup/cgroup.subtree_control", enable.trim())
        .context("enabling cgroup controllers at the root")?;
    fs::create_dir_all(SYSTEM_CGROUP).context("creating the system cgroup")?;
    fs::write(format!("{SYSTEM_CGROUP}/cgroup.subtree_control"), enable.trim())
        .context("enabling cgroup controllers for system")?;
    Ok(())
}

/// Unmounts every filesystem on a disk, last mounted first, so the state and
/// games disks are clean on the next boot. Runs after every process is gone,
/// so nothing holds them. Memory-backed mounts need nothing and stay.
pub fn teardown() {
    // SAFETY: plain syscall.
    unsafe { libc::sync() };
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let on_disks: Vec<&str> = mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (source, target) = (f.next()?, f.next()?);
            (source.starts_with("/dev/") && target != "/").then_some(target)
        })
        .collect();
    for target in on_disks.iter().rev() {
        if let Err(e) = umount(target, 0) {
            warn!("unmounting {target}: {e}");
        }
    }
}
