//! Finding the boot medium: the FAT partition, on a USB stick or an internal
//! disk, that holds each slot's kernel and root image.

use std::fs;
use std::io;
use std::path::PathBuf;

/// GPT partition name of the boot medium.
pub const PARTNAME: &str = "NESLINA";

/// A partition as the kernel names it, `sda1` or `nvme0n1p1`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Partition {
    pub name: String,
}

impl Partition {
    pub fn dev(&self) -> PathBuf {
        PathBuf::from("/dev").join(&self.name)
    }

    pub fn sys(&self) -> PathBuf {
        PathBuf::from("/sys/class/block").join(&self.name)
    }

    /// Its number in the partition table, counting from 1.
    pub fn number(&self) -> io::Result<u32> {
        read_trimmed(self.sys().join("partition"))?
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad partition number"))
    }

    /// The whole disk it is on, `sda` or `nvme0n1`.
    pub fn disk(&self) -> io::Result<String> {
        let parent = fs::canonicalize(self.sys())?
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_owned);
        parent.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "partition has no parent disk"))
    }
}

/// Every partition with this GPT name, sorted.
pub fn find(partname: &str) -> io::Result<Vec<Partition>> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/sys/class/block")? {
        let Ok(uevent) = fs::read_to_string(entry?.path().join("uevent")) else { continue };
        let mut name = None;
        let mut devname = None;
        for line in uevent.lines() {
            match line.split_once('=') {
                Some(("PARTNAME", v)) => name = Some(v),
                Some(("DEVNAME", v)) => devname = Some(v),
                _ => {}
            }
        }
        if let (Some(n), Some(d)) = (name, devname)
            && n == partname
        {
            found.push(Partition { name: d.to_owned() });
        }
    }
    found.sort();
    Ok(found)
}

pub fn read_trimmed(path: impl AsRef<std::path::Path>) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_owned())
}

/// The slot this kernel was built as, from `neslina.slot=` on its command line.
pub fn current_slot() -> io::Result<String> {
    let cmdline = fs::read_to_string("/proc/cmdline")?;
    cmdline
        .split_whitespace()
        .find_map(|arg| arg.strip_prefix("neslina.slot="))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no neslina.slot= on the command line"))
}

/// Mounts the medium read-only at `at` for the length of `f`, then unmounts
/// it whatever `f` returned.
pub fn with_mounted<T>(part: &Partition, at: &std::path::Path, f: impl FnOnce(&std::path::Path) -> T) -> io::Result<T> {
    with_mount(part, at, libc::MS_RDONLY, f)
}

/// The same, writable, for putting slot files and `boot.conf` on it.
/// Everything is flushed before it is unmounted.
pub fn with_mounted_rw<T>(
    part: &Partition,
    at: &std::path::Path,
    f: impl FnOnce(&std::path::Path) -> T,
) -> io::Result<T> {
    with_mount(part, at, 0, f)
}

fn with_mount<T>(
    part: &Partition,
    at: &std::path::Path,
    extra: libc::c_ulong,
    f: impl FnOnce(&std::path::Path) -> T,
) -> io::Result<T> {
    fs::create_dir_all(at)?;
    let flags = extra | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    crate::sys::mount(part.dev().to_str().unwrap_or_default(), at, "vfat", flags, None)?;
    let out = f(at);
    if let Err(e) = crate::sys::umount(at, 0) {
        crate::warn!("unmounting {}: {e}", at.display());
    }
    Ok(out)
}
