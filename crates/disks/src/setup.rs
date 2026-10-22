//! `neslina/setup.toml` on the boot medium: what the owner decided before the
//! first boot. Today only the disks; the setup UI's password comes later.
//!
//! ```toml
//! [disks]
//! state = { serial = "S6B0NL0W123456" }
//! games = { name = "sdb", wipe = true }
//! ```
//!
//! Naming a disk here is consent to wipe it, unless it already has
//! partitions; then `wipe = true` has to say so too.

use serde::Deserialize;

use crate::blockdev::Disk;

#[derive(Debug, Default, Deserialize)]
pub struct Setup {
    pub disks: Option<Disks>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disks {
    pub state: Select,
    pub games: Option<Select>,
}

/// One disk, by serial (stable) or kernel name (can change between boots).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    pub serial: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
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
        if !disk.partitions.is_empty() && !self.wipe {
            return Err(format!(
                "{role}: {} has partitions ({}); add wipe = true to erase it",
                disk.name,
                disk.partitions.join(", ")
            ));
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
        assert_eq!(
            select("[disks]\nstate = { serial = \"neslina-state\" }").resolve("state", &disks).unwrap().name,
            "vda"
        );
        assert_eq!(select("[disks]\nstate = { name = \"vda\" }").resolve("state", &disks).unwrap().name, "vda");
    }

    #[test]
    fn refuses_the_medium_unknown_disks_and_unconfirmed_wipes() {
        let disks = [disk("sda", "STICK", &["sda1"], true), disk("vda", "OLD", &["vda1"], false)];
        assert!(select("[disks]\nstate = { name = \"sda\", wipe = true }").resolve("state", &disks).is_err());
        assert!(select("[disks]\nstate = { serial = \"missing\" }").resolve("state", &disks).is_err());
        assert!(select("[disks]\nstate = { name = \"vda\" }").resolve("state", &disks).is_err());
        assert!(select("[disks]\nstate = { name = \"vda\", wipe = true }").resolve("state", &disks).is_ok());
        assert!(select("[disks]\nstate = { name = \"vda\", serial = \"OLD\" }").resolve("state", &disks).is_err());
    }

    #[test]
    fn setup_without_disks_is_fine() {
        assert!(toml::from_str::<Setup>("").unwrap().disks.is_none());
    }
}
