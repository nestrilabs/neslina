//! The filesystems a read-only root needs before anything runs.

use std::fs;

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

/// Nothing on the root is writable and persistent yet, so this only flushes
/// and detaches what can be. Disks join when setup does.
pub fn teardown() {
    // SAFETY: plain syscall.
    unsafe { libc::sync() };
    for target in ["/tmp", "/dev/shm"] {
        if let Err(e) = umount(target, libc::MNT_DETACH) {
            warn!("unmounting {target}: {e}");
        }
    }
}
