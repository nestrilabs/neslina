//! The disks service. Runs once per boot:
//!
//! 1. If neslina's state partition exists, mount it at `/var`, and the games
//!    partition, if there is one, below it.
//! 2. Otherwise, if `neslina/setup.toml` on the boot medium assigns disks,
//!    make them (wipe, partition, format XFS) and mount them.
//! 3. Otherwise wait for setup to assign them. Nothing needs them yet.
//!
//! A mistake in setup.toml is logged and the service exits cleanly: retrying
//! cannot fix it before the owner edits the file.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use neslina_common::medium;
use neslina_common::sys::mount;
use neslina_common::{error, info};
use neslina_disks::blockdev;
use neslina_disks::setup::Setup;
use neslina_disks::{GAMES, STATE};

const STATE_MOUNT: &str = "/var";
const GAMES_MOUNT: &str = "/var/neslina/games";
const MEDIUM_MOUNT: &str = "/run/neslina/medium";
const SETUP: &str = "neslina/setup.toml";

fn main() {
    neslina_common::kmsg::init("disks");
    if let Err(e) = run() {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if let Some(state) = find(STATE)? {
        return mount_all(&state, find(GAMES)?.as_deref());
    }

    let part = medium::find(medium::PARTNAME)?
        .into_iter()
        .next()
        .with_context(|| format!("no partition named {}", medium::PARTNAME))?;
    let text = medium::with_mounted(&part, Path::new(MEDIUM_MOUNT), |dir| fs::read_to_string(dir.join(SETUP)))
        .context("mounting the boot medium")?;
    let setup: Setup = match text {
        Ok(t) => match toml::from_str(&t) {
            Ok(s) => s,
            Err(e) => {
                error!("{SETUP}: {e}");
                return Ok(());
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Setup::default(),
        Err(e) => return Err(e).context(format!("reading {SETUP}")),
    };
    let Some(wanted) = setup.disks else {
        info!("no state disk yet; waiting for setup to assign one");
        return Ok(());
    };

    let disks = blockdev::list().context("listing disks")?;
    let state = match wanted.state.resolve("state", &disks) {
        Ok(d) => d,
        Err(e) => {
            error!("{SETUP}: {e}");
            return Ok(());
        }
    };
    let games = match wanted.games.as_ref().map(|g| g.resolve("games", &disks)).transpose() {
        Ok(Some(g)) if g.name == state.name => {
            error!("{SETUP}: state and games are the same disk, {}", g.name);
            return Ok(());
        }
        Ok(g) => g,
        Err(e) => {
            error!("{SETUP}: {e}");
            return Ok(());
        }
    };

    info!("making {} ({} GiB) the state disk", state.name, state.size >> 30);
    let state_dev =
        blockdev::make_xfs_disk(state, STATE, "state").with_context(|| format!("preparing {}", state.name))?;
    let games_dev = match games {
        Some(g) => {
            info!("making {} ({} GiB) the games disk", g.name, g.size >> 30);
            Some(blockdev::make_xfs_disk(g, GAMES, "games").with_context(|| format!("preparing {}", g.name))?)
        }
        None => None,
    };
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    mount_all(&name(&state_dev), games_dev.as_deref().map(name).as_deref())
}

/// The kernel name of the partition with this GPT name, if exactly one has it.
fn find(partname: &str) -> Result<Option<String>> {
    let found = medium::find(partname).context("listing partitions")?;
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.name.clone())),
        many => anyhow::bail!(
            "{} partitions are named {partname}: {}; remove all but one",
            many.len(),
            many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn mount_all(state: &str, games: Option<&str>) -> Result<()> {
    // Not noexec: builds run what they compile from here.
    let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOATIME;
    mount(&format!("/dev/{state}"), STATE_MOUNT, "xfs", flags, None)
        .with_context(|| format!("mounting {state} at {STATE_MOUNT}"))?;
    info!("state disk {state} mounted at {STATE_MOUNT}");
    if let Some(games) = games {
        fs::create_dir_all(GAMES_MOUNT)?;
        mount(&format!("/dev/{games}"), GAMES_MOUNT, "xfs", flags, None)
            .with_context(|| format!("mounting {games} at {GAMES_MOUNT}"))?;
        info!("games disk {games} mounted at {GAMES_MOUNT}");
    }
    Ok(())
}
