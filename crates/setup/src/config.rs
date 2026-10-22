//! Reading and changing `neslina/setup.toml` on the boot medium.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use neslina_common::medium;
use neslina_disks::setup::{Disks, Setup};

pub const SETUP: &str = "neslina/setup.toml";
const MOUNT: &str = "/run/neslina/medium-setup";

fn medium() -> Result<medium::Partition> {
    medium::find(medium::PARTNAME)?
        .into_iter()
        .next()
        .with_context(|| format!("no partition named {}", medium::PARTNAME))
}

pub fn read() -> Result<Setup> {
    let text = medium::with_mounted(&medium()?, Path::new(MOUNT), |dir| fs::read_to_string(dir.join(SETUP)))
        .context("mounting the boot medium")?;
    match text {
        Ok(t) => toml::from_str(&t).map_err(|e| anyhow::anyhow!("{SETUP}: {}", e.message())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Setup::default()),
        Err(e) => Err(e).context(format!("reading {SETUP}")),
    }
}

/// Changes setup.toml as a table, keeping every key it does not touch.
/// Comments are not kept; the file is the owner's only until setup starts.
fn edit(change: impl FnOnce(&mut toml::Table) -> Result<()>) -> Result<()> {
    medium::with_mounted_rw(&medium()?, Path::new(MOUNT), |dir| -> Result<()> {
        let path = dir.join(SETUP);
        let mut table: toml::Table = match fs::read_to_string(&path) {
            Ok(t) => toml::from_str(&t).map_err(|e| anyhow::anyhow!("{SETUP}: {}", e.message()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            Err(e) => return Err(e).context(format!("reading {SETUP}")),
        };
        change(&mut table)?;
        fs::write(&path, toml::to_string(&table)?).with_context(|| format!("writing {SETUP}"))?;
        fs::File::open(&path)?.sync_all()?;
        Ok(())
    })
    .context("mounting the boot medium writable")?
}

/// Replaces the first-login password with the owner's own, hashed: the stick
/// keeps no readable password after this.
pub fn set_password_hash(hash: &str) -> Result<()> {
    edit(|t| {
        t.remove("password");
        t.insert("password_hash".into(), toml::Value::String(hash.to_owned()));
        Ok(())
    })
}

pub fn set_disks(disks: &Disks) -> Result<()> {
    let value = toml::Value::try_from(disks)?;
    edit(|t| {
        t.insert("disks".into(), value);
        Ok(())
    })
}
