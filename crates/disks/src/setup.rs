//! `neslina/setup.toml` on the boot medium: what the owner decided before the
//! first boot. The setup UI's password, and the disks.
//!
//! ```toml
//! password = "written by the image writer"   # replaced by password_hash on first login
//! ```
//!
//! ```toml
//! [disks]
//! state = { serial = "S6B0NL0W123456", wipe = true }
//! games = { name = "sdb", wipe = true }
//! ```
//!
//! Nothing is chosen or wiped by default. Each entry names exactly one disk,
//! by serial or kernel name, and says `wipe` outright: a missing `wipe` is an
//! error, not `false`. A disk is erased only with `wipe = true`, empty or
//! not. Without that, the host keeps running from the stick and leaves every
//! internal disk alone.

use serde::{Deserialize, Serialize};

use crate::blockdev::Disk;

#[derive(Debug, Default, Deserialize)]
pub struct Setup {
    /// The first-login password, as the image writer generated it.
    pub password: Option<String>,
    /// What replaces it once the owner sets their own (argon2, PHC string).
    pub password_hash: Option<String>,
    pub disks: Option<Disks>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Disks {
    pub state: Select,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub games: Option<Select>,
}

/// One disk, by serial (stable) or kernel name (can change between boots).
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Required: an entry must say whether its disk may be erased.
    pub wipe: bool,
}

impl Select {
    /// The one disk this selects, ready to wipe, or why not.
    pub fn resolve<'a>(&self, role: &str, disks: &'a [Disk]) -> Result<&'a Disk, String> {
        let found: Vec<&Disk> = match (&self.serial, &self.name) {
            (Some(s), None) => disks.iter().filter(|d| d.serial.as_deref() == Some(s)).collect(),
            (None, Some(n)) => disks.iter().filter(|d| &d.name == n).collect(),
            _ => return Err(format!("{role}: give exactly one of serial or name")),
        };
        let disk = match found.as_slice() {
            [one] => *one,
            [] => return Err(format!("{role}: no disk matches {}", self.describe())),
            _ => return Err(format!("{role}: more than one disk matches {}", self.describe())),
        };
        if disk.holds_medium {
            return Err(format!("{role}: {} holds the boot medium", disk.name));
        }
        if !self.wipe {
            let contents = if disk.partitions.is_empty() {
                "no partitions".to_owned()
            } else {
                format!("partitions {}", disk.partitions.join(", "))
            };
            return Err(format!("{role}: {} ({contents}) is not erased without wipe = true", disk.name));
        }
        Ok(disk)
    }

    fn describe(&self) -> String {
        match (&self.serial, &self.name) {
            (Some(s), _) => format!("serial {s:?}"),
            (_, Some(n)) => format!("name {n:?}"),
            _ => "nothing".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockdev::Transport;

    fn disk(name: &str, serial: &str, partitions: &[&str], holds_medium: bool) -> Disk {
        Disk {
            name: name.into(),
            size: 64 << 30,
            block: 512,
            model: None,
            serial: Some(serial.into()),
            transport: Transport::Virtio,
            removable: false,
            partitions: partitions.iter().map(|p| p.to_string()).collect(),
            holds_medium,
        }
    }

    fn select(text: &str) -> Select {
        toml::from_str::<Setup>(text).unwrap().disks.unwrap().state
    }

    #[test]
    fn resolves_by_serial_or_name() {
        let disks = [disk("sda", "STICK", &["sda1"], true), disk("vda", "neslina-state", &[], false)];
        let by_serial = "[disks]\nstate = { serial = \"neslina-state\", wipe = true }";
        assert_eq!(select(by_serial).resolve("state", &disks).unwrap().name, "vda");
        let by_name = "[disks]\nstate = { name = \"vda\", wipe = true }";
        assert_eq!(select(by_name).resolve("state", &disks).unwrap().name, "vda");
    }

    #[test]
    fn never_wipes_without_saying_so_even_an_empty_disk() {
        let disks = [disk("vda", "EMPTY", &[], false)];
        assert!(select("[disks]\nstate = { name = \"vda\", wipe = false }").resolve("state", &disks).is_err());
    }

    #[test]
    fn nothing_is_left_to_a_default() {
        // No disk named at all, and no wipe stated: both are errors.
        let disks = [disk("vda", "EMPTY", &[], false)];
        assert!(select("[disks]\nstate = { wipe = true }").resolve("state", &disks).is_err());
        assert!(toml::from_str::<Setup>("[disks]\nstate = { name = \"vda\" }").is_err());
        assert!(toml::from_str::<Setup>("[disks]\nstate = {}").is_err());
    }

    #[test]
    fn refuses_the_medium_unknown_disks_and_unconfirmed_wipes() {
        let disks = [disk("sda", "STICK", &["sda1"], true), disk("vda", "OLD", &["vda1"], false)];
        assert!(select("[disks]\nstate = { name = \"sda\", wipe = true }").resolve("state", &disks).is_err());
        assert!(select("[disks]\nstate = { serial = \"missing\", wipe = true }").resolve("state", &disks).is_err());
        assert!(select("[disks]\nstate = { name = \"vda\", wipe = false }").resolve("state", &disks).is_err());
        assert!(select("[disks]\nstate = { name = \"vda\", wipe = true }").resolve("state", &disks).is_ok());
        let both = "[disks]\nstate = { name = \"vda\", serial = \"OLD\", wipe = true }";
        assert!(select(both).resolve("state", &disks).is_err());
    }

    #[test]
    fn setup_without_disks_is_fine() {
        assert!(toml::from_str::<Setup>("").unwrap().disks.is_none());
    }
}
