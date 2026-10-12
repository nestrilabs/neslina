//! The boot service. It runs once per boot, after init is up:
//!
//! 1. If a slot was on trial, it settles the outcome. Booted into it: wait
//!    until it counts as healthy, then make it the default. Booted into
//!    anything else: the trial slot panicked or hung and the firmware fell
//!    back, so record it as failed.
//! 2. If the boot medium asks for a slot other than the running one, it
//!    registers that slot, sets `BootNext` to it and reboots. A build of a
//!    slot that already failed is not tried again.
//!
//! The wanted slot is `want=<slot>` in `neslina/boot.conf` on the boot medium.
//! The trial's state is neslina's own UEFI variable, so it survives the
//! fallback without anything written to disk.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use neslina_boot::efivar::{self, GLOBAL, NESLINA};
use neslina_boot::gpt::{self, GptPartition};
use neslina_boot::loadopt::LoadOption;
use neslina_common::medium::{self, Partition};
use neslina_common::sys::{mount, umount};
use neslina_common::{error, info, warn};
use sha2::{Digest, Sha256};

const MOUNT: &str = "/run/neslina/medium";
const CONF: &str = "neslina/boot.conf";
const TRIAL_VAR: &str = "NeslinaTrial";
/// How long a slot on trial must run before it becomes the default. A
/// stand-in until something above init can report health (neslet, later).
const HEALTHY_AFTER: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
enum TrialState {
    Trying,
    Failed,
}

#[derive(Clone, Debug)]
struct Trial {
    slot: String,
    /// sha256 of the slot's EFI file. A rebuilt slot gets a new trial.
    efi: String,
    state: TrialState,
}

impl Trial {
    fn read() -> Result<Option<Self>> {
        let Some(raw) = efivar::read(TRIAL_VAR, NESLINA).context("reading the trial variable")? else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(&raw);
        let (mut slot, mut efi, mut state) = (None, None, None);
        for line in text.lines() {
            match line.split_once('=') {
                Some(("slot", v)) => slot = Some(v.to_owned()),
                Some(("efi", v)) => efi = Some(v.to_owned()),
                Some(("state", "trying")) => state = Some(TrialState::Trying),
                Some(("state", "failed")) => state = Some(TrialState::Failed),
                _ => {}
            }
        }
        match (slot, efi, state) {
            (Some(slot), Some(efi), Some(state)) => Ok(Some(Trial { slot, efi, state })),
            _ => {
                warn!("ignoring a malformed trial variable: {text:?}");
                Ok(None)
            }
        }
    }

    fn write(&self) -> Result<()> {
        let state = match self.state {
            TrialState::Trying => "trying",
            TrialState::Failed => "failed",
        };
        let text = format!("slot={}\nefi={}\nstate={state}\n", self.slot, self.efi);
        efivar::write(TRIAL_VAR, NESLINA, text.as_bytes()).context("writing the trial variable")
    }
}

struct Wanted {
    slot: String,
    efi: String,
}

fn main() {
    neslina_common::kmsg::init("boot");
    if let Err(e) = run() {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if !Path::new(efivar::DIR).join(format!("BootCurrent-{GLOBAL}")).exists() {
        warn!("no UEFI variables; slots cannot be tried or promoted on this boot");
        return Ok(());
    }
    let current = medium::current_slot()?;
    let part = medium::find(medium::PARTNAME)?
        .into_iter()
        .next()
        .with_context(|| format!("no partition named {}", medium::PARTNAME))?;
    let gpt = gpt::lookup(&part).with_context(|| format!("reading {}'s GPT entry", part.name))?;

    if let Some(t) = Trial::read()?
        && t.state == TrialState::Trying
    {
        if t.slot == current {
            info!("slot {current} is on trial; it becomes the default after {}s", HEALTHY_AFTER.as_secs());
            std::thread::sleep(HEALTHY_AFTER);
            promote(&gpt, &current)?;
            efivar::delete(TRIAL_VAR, NESLINA).context("clearing the trial variable")?;
            info!("slot {current} passed its trial and is now the default");
        } else {
            error!("slot {} failed its trial; running slot {current} instead", t.slot);
            Trial { state: TrialState::Failed, ..t }.write()?;
        }
    }

    let Some(want) = read_wanted(&part)? else { return Ok(()) };
    if want.slot == current {
        return Ok(());
    }
    if let Some(t) = Trial::read()?
        && t.state == TrialState::Failed
        && t.slot == want.slot
        && t.efi == want.efi
    {
        warn!("not trying slot {} again: this build of it already failed its trial", want.slot);
        return Ok(());
    }

    let number = register(&gpt, &want.slot)?;
    Trial { slot: want.slot.clone(), efi: want.efi, state: TrialState::Trying }.write()?;
    efivar::write("BootNext", GLOBAL, &number.to_le_bytes()).context("setting BootNext")?;
    info!("trying slot {} ({}) on the next boot; rebooting", want.slot, efivar::boot_name(number));
    // SAFETY: plain syscall; init reboots in order on SIGTERM.
    unsafe { libc::kill(1, libc::SIGTERM) };
    Ok(())
}

/// Reads `want=` from the boot medium and hashes that slot's EFI file. The
/// medium is mounted read-only only for as long as this takes.
fn read_wanted(part: &Partition) -> Result<Option<Wanted>> {
    std::fs::create_dir_all(MOUNT)?;
    let dev = part.dev();
    let flags = libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    mount(dev.to_str().unwrap_or_default(), MOUNT, "vfat", flags, None)
        .with_context(|| format!("mounting {} at {MOUNT}", dev.display()))?;
    let result = (|| {
        let conf = match std::fs::read_to_string(Path::new(MOUNT).join(CONF)) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context(format!("reading {CONF}")),
        };
        let Some(slot) = conf.lines().find_map(|l| l.trim().strip_prefix("want=")).map(str::to_owned) else {
            return Ok(None);
        };
        if !slot.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') || slot.is_empty() {
            bail!("{CONF} names slot {slot:?}; slot names are lowercase letters, digits and dashes");
        }
        let efi = sha256_file(&Path::new(MOUNT).join("neslina").join(format!("{slot}.efi")))?;
        Ok(Some(Wanted { slot, efi }))
    })();
    if let Err(e) = umount(MOUNT, 0) {
        warn!("unmounting {MOUNT}: {e}");
    }
    result
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn slot_file(slot: &str) -> String {
    format!(r"\neslina\{slot}.efi")
}

/// The `Boot####` entry for this slot on this partition, if there is one.
fn find_entry(gpt: &GptPartition, slot: &str) -> Result<Option<u16>> {
    let file = slot_file(slot);
    for n in efivar::boot_entries()? {
        let Some(raw) = efivar::read(&efivar::boot_name(n), GLOBAL)? else { continue };
        let Some(opt) = LoadOption::decode(&raw) else { continue };
        if opt.partition.is_some_and(|p| p.guid == gpt.guid) && opt.file.is_some_and(|f| f.eq_ignore_ascii_case(&file))
        {
            return Ok(Some(n));
        }
    }
    Ok(None)
}

/// Makes sure the slot has a current `Boot####` entry and returns its number.
fn register(gpt: &GptPartition, slot: &str) -> Result<u16> {
    let opt = LoadOption { description: format!("neslina {slot}"), partition: Some(*gpt), file: Some(slot_file(slot)) };
    let number = match find_entry(gpt, slot)? {
        Some(n) => n,
        None => {
            let used = efivar::boot_entries()?;
            (0..=u16::MAX).find(|n| !used.contains(n)).context("no free Boot#### number")?
        }
    };
    let name = efivar::boot_name(number);
    let encoded = opt.encode();
    if efivar::read(&name, GLOBAL)?.as_deref() != Some(&encoded[..]) {
        efivar::write(&name, GLOBAL, &encoded).with_context(|| format!("writing {name}"))?;
        info!("registered slot {slot} as {name}");
    }
    Ok(number)
}

/// Moves the slot's entry to the front of `BootOrder`.
fn promote(gpt: &GptPartition, slot: &str) -> Result<()> {
    let Some(number) = find_entry(gpt, slot)? else {
        bail!("slot {slot} has no boot entry to promote");
    };
    let mut order = efivar::read_u16s("BootOrder")?;
    order.retain(|&n| n != number);
    order.insert(0, number);
    efivar::write_u16s("BootOrder", &order).context("writing BootOrder")
}
