//! The installer service. On the rescue slot, once the state disk is mounted,
//! it turns the install bundle on the boot medium into the host's own native
//! slot:
//!
//! 1. Copies `neslina/install/{sysroot,source}.erofs` to the state disk and
//!    mounts both read-only.
//! 2. Runs `neslina build --cpu native --slot a` against them. The build log
//!    goes to the state disk, not the kernel log.
//! 3. Puts `a.efi` and `a.erofs` on the boot medium with `want=a`, then runs
//!    the boot service, which tries the slot with `BootNext` and reboots.
//!
//! It does nothing if the host is not on rescue, has no state disk yet, has no
//! bundle, or already wants a slot. A failed build is logged and not retried:
//! the same inputs would fail the same way.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use neslina_common::medium;
use neslina_common::sys::{is_mountpoint, mount};
use neslina_common::{error, info};

const SLOT: &str = "a";
const STATE: &str = "/var";
const HOME: &str = "/var/neslina";
const SYSROOT_MOUNT: &str = "/run/neslina/sysroot";
const SOURCE_MOUNT: &str = "/run/neslina/source";
const MEDIUM_MOUNT: &str = "/run/neslina/medium-install";
const BUNDLE: &[&str] = &["sysroot.erofs", "source.erofs"];
/// Lines of a failed build's log repeated in the kernel log.
const LOG_TAIL: usize = 20;

fn main() {
    neslina_common::kmsg::init("install");
    if let Err(e) = run() {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if medium::current_slot()? != "rescue" {
        return Ok(());
    }
    if !is_mountpoint(STATE) {
        info!("no state disk yet; nothing to install until setup assigns one");
        return Ok(());
    }
    let part = medium::find(medium::PARTNAME)?
        .into_iter()
        .next()
        .with_context(|| format!("no partition named {}", medium::PARTNAME))?;

    let home = Path::new(HOME);
    let install = home.join("install");
    fs::create_dir_all(&install)?;
    let found = medium::with_mounted(&part, Path::new(MEDIUM_MOUNT), |dir| -> Result<bool> {
        if fs::read_to_string(dir.join("neslina/boot.conf")).is_ok_and(|c| c.contains("want=")) {
            return Ok(false);
        }
        let bundle = dir.join("neslina/install");
        if !BUNDLE.iter().all(|f| bundle.join(f).exists()) {
            info!("no install bundle on the boot medium");
            return Ok(false);
        }
        for f in BUNDLE {
            copy_if_changed(&bundle.join(f), &install.join(f))?;
        }
        Ok(true)
    })
    .context("mounting the boot medium")??;
    if !found {
        return Ok(());
    }

    for (image, at) in [("sysroot.erofs", SYSROOT_MOUNT), ("source.erofs", SOURCE_MOUNT)] {
        if !is_mountpoint(at) {
            fs::create_dir_all(at)?;
            let file = install.join(image);
            mount(file.to_str().unwrap_or_default(), at, "erofs", libc::MS_RDONLY | libc::MS_NODEV, None)
                .with_context(|| format!("mounting {image}"))?;
        }
    }

    let log_dir = home.join("log");
    fs::create_dir_all(&log_dir)?;
    let log = log_dir.join(format!("build-{SLOT}.log"));
    let version = fs::read_to_string(Path::new(SOURCE_MOUNT).join("repo/VERSION")).unwrap_or_default();
    info!("building slot {SLOT} natively from bundle {}; log in {}", version.trim(), log.display());
    let out = home.join("out");
    let log_file = File::create(&log)?;
    let status = Command::new("/usr/bin/neslina")
        .args(["--repo", &format!("{SOURCE_MOUNT}/repo"), "--store", &format!("{SOURCE_MOUNT}/store")])
        .args(["build", "--cpu", "native", "--slot", SLOT, "--sysroot", SYSROOT_MOUNT])
        .arg("--work")
        .arg(home.join("work"))
        .arg("--out")
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file)
        .status()
        .context("running neslina build")?;
    if !status.success() {
        error!("building slot {SLOT} failed ({status}); the end of {}:", log.display());
        for line in tail(&log, LOG_TAIL) {
            error!("{line}");
        }
        return Ok(());
    }
    info!("built slot {SLOT}");

    let built = out.join(SLOT);
    medium::with_mounted_rw(&part, Path::new(MEDIUM_MOUNT), |dir| -> Result<()> {
        for ext in ["efi", "erofs"] {
            let name = format!("{SLOT}.{ext}");
            fs::copy(built.join(&name), dir.join("neslina").join(&name)).with_context(|| format!("writing {name}"))?;
            File::open(dir.join("neslina").join(&name))?.sync_all()?;
        }
        let conf = dir.join("neslina/boot.conf");
        fs::write(&conf, format!("want={SLOT}\n"))?;
        File::open(&conf)?.sync_all()?;
        Ok(())
    })
    .context("mounting the boot medium writable")??;
    info!("slot {SLOT} is on the boot medium; handing over to the boot service");

    // The boot service already ran this boot, before there was a slot to want.
    let status = Command::new("/usr/lib/neslina/neslina-boot").status().context("running the boot service")?;
    if !status.success() {
        bail!("the boot service failed ({status})");
    }
    Ok(())
}

/// Copies unless the destination already has the same size: the bundle is
/// large, and an interrupted install should not copy it all again.
fn copy_if_changed(from: &Path, to: &PathBuf) -> Result<()> {
    let size = fs::metadata(from)?.len();
    if fs::metadata(to).is_ok_and(|m| m.len() == size) {
        return Ok(());
    }
    info!("copying {} ({} MiB) to the state disk", from.display(), size >> 20);
    let partial = to.with_extension("partial");
    fs::copy(from, &partial).with_context(|| format!("copying {}", from.display()))?;
    File::open(&partial)?.sync_all()?;
    fs::rename(&partial, to)?;
    Ok(())
}

fn tail(path: &Path, n: usize) -> Vec<String> {
    let Ok(f) = File::open(path) else { return Vec::new() };
    let lines: Vec<String> = BufReader::new(f).lines().map_while(Result::ok).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}
