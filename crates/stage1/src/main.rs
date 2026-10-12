//! Stage 1: the program in each slot's built-in initramfs.
//!
//! It finds the boot medium, copies this slot's root image into RAM while
//! hashing it, checks the hash the kernel was built with, mounts the copy and
//! hands over to init. The hash catches a corrupt or mismatched image (a
//! failing USB stick, a half-written update), not an attacker.
//!
//! Any error ends this process. As PID 1 that panics the kernel, and `panic=`
//! on the command line reboots the machine, so the firmware moves on to the
//! previous slot.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use neslina_common::sys::{self, mount, umount};
use neslina_common::{error, info, warn};
use sha2::{Digest, Sha256};

/// GPT partition name of the boot medium, on a USB stick or an internal disk.
const MEDIUM_PARTNAME: &str = "NESLINA";
/// Written into the initramfs by the image build: which slot this kernel is,
/// and the hash of the root image built with it.
const SLOT_FILE: &str = "/neslina/slot";
const MEDIUM: &str = "/medium";
const NEWROOT: &str = "/newroot";
/// The RAM copy lives on its own ramfs. erofs only mounts a file whose
/// filesystem can read it a folio at a time (`read_folio`), which ramfs can and
/// tmpfs, the initramfs's own filesystem, cannot. The mount stays alive after
/// the switch because the root mount still refers to the file.
const RAM: &str = "/ram";
const RAM_IMAGE: &str = "/ram/root.erofs";
const MEDIUM_TIMEOUT: Duration = Duration::from_secs(30);

struct Slot {
    name: String,
    sha256: String,
}

fn main() {
    neslina_common::kmsg::init("stage1");
    if let Err(e) = run() {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    mount("devtmpfs", "/dev", "devtmpfs", libc::MS_NOSUID, None).context("mounting /dev")?;
    mount("proc", "/proc", "proc", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None)
        .context("mounting /proc")?;
    mount("sysfs", "/sys", "sysfs", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None)
        .context("mounting /sys")?;

    let slot = read_slot()?;
    info!("slot {}, looking for the boot medium", slot.name);

    let dev = wait_for_medium()?;
    mount(
        dev.to_str().unwrap_or_default(),
        MEDIUM,
        "vfat",
        libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        None,
    )
    .with_context(|| format!("mounting {} at {MEDIUM}", dev.display()))?;

    mount("ramfs", RAM, "ramfs", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, Some("mode=0700"))
        .context("mounting the ramfs for the root image")?;
    let image = Path::new(MEDIUM).join("neslina").join(format!("{}.erofs", slot.name));
    let started = Instant::now();
    let (size, sha256) = copy_and_hash(&image, Path::new(RAM_IMAGE))?;
    umount(MEDIUM, 0).context("unmounting the boot medium")?;
    if sha256 != slot.sha256 {
        bail!("{} has hash {sha256}, this kernel expects {}", image.display(), slot.sha256);
    }
    info!(
        "copied {} ({} MiB) into RAM in {} ms, hash matches",
        image.display(),
        size >> 20,
        started.elapsed().as_millis()
    );

    mount(RAM_IMAGE, NEWROOT, "erofs", libc::MS_RDONLY | libc::MS_NODEV, None).context("mounting the root image")?;
    for dir in ["/dev", "/proc", "/sys"] {
        mount(dir, Path::new(NEWROOT).join(&dir[1..]), "", libc::MS_MOVE, None)
            .with_context(|| format!("moving {dir} into the new root"))?;
    }

    switch_root()
}

fn read_slot() -> Result<Slot> {
    let text = fs::read_to_string(SLOT_FILE).with_context(|| format!("reading {SLOT_FILE}"))?;
    let (mut name, mut sha256) = (None, None);
    for line in text.lines() {
        match line.split_once('=') {
            Some(("slot", v)) => name = Some(v.to_owned()),
            Some(("sha256", v)) => sha256 = Some(v.to_owned()),
            _ => {}
        }
    }
    match (name, sha256) {
        (Some(name), Some(sha256)) => Ok(Slot { name, sha256 }),
        _ => bail!("{SLOT_FILE} needs both slot= and sha256="),
    }
}

/// USB storage probes asynchronously, so the medium can appear a few seconds
/// after the kernel hands over.
fn wait_for_medium() -> Result<PathBuf> {
    let deadline = Instant::now() + MEDIUM_TIMEOUT;
    loop {
        let found = find_partitions(MEDIUM_PARTNAME)?;
        if let Some(first) = found.first() {
            if found.len() > 1 {
                // Picking by firmware boot entry comes with the A/B work.
                warn!("{} partitions named {MEDIUM_PARTNAME}, using {}", found.len(), first.display());
            }
            info!("boot medium is {}", first.display());
            return Ok(first.clone());
        }
        if Instant::now() >= deadline {
            bail!("no partition named {MEDIUM_PARTNAME} after {}s", MEDIUM_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn find_partitions(partname: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/sys/class/block").context("listing /sys/class/block")? {
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
            found.push(Path::new("/dev").join(d));
        }
    }
    found.sort();
    Ok(found)
}

fn copy_and_hash(from: &Path, to: &Path) -> Result<(u64, String)> {
    let mut src = File::open(from).with_context(|| format!("opening {}", from.display()))?;
    let mut dst = File::create(to).with_context(|| format!("creating {}", to.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf).with_context(|| format!("reading {}", from.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n]).with_context(|| format!("writing {}", to.display()))?;
        total += n as u64;
    }
    let hex = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok((total, hex))
}

/// The initramfs is the kernel's rootfs and cannot be pivoted away from, so
/// this is the classic move-and-chroot. Its files stay, including the RAM copy
/// the new root is mounted from.
fn switch_root() -> Result<()> {
    std::env::set_current_dir(NEWROOT)?;
    mount(".", "/", "", libc::MS_MOVE, None).context("moving the new root onto /")?;
    sys::chroot(".").context("chroot into the new root")?;
    std::env::set_current_dir("/")?;
    let err = Command::new("/sbin/init").arg0("init").exec();
    Err(err).context("executing /sbin/init")
}
