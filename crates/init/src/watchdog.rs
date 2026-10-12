//! The hardware watchdog. While init runs it pings; if init or the kernel
//! hangs, the board resets, and a slot on trial falls back to the previous one.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use neslina_common::{info, warn};

const DEVICE: &str = "/dev/watchdog0";
const TIMEOUT: Duration = Duration::from_secs(30);
const PING_EVERY: Duration = Duration::from_secs(10);
/// `_IOWR('W', 6, int)` from linux/watchdog.h.
const WDIOC_SETTIMEOUT: libc::c_ulong = 0xC004_5706;

pub struct Watchdog {
    file: File,
    last_ping: Instant,
}

impl Watchdog {
    /// Arms the watchdog if the machine has one.
    pub fn open() -> Option<Self> {
        let file = match OpenOptions::new().write(true).open(DEVICE) {
            Ok(f) => f,
            Err(e) => {
                warn!("no watchdog ({DEVICE}: {e}); a hang will need a manual reset");
                return None;
            }
        };
        let mut secs = TIMEOUT.as_secs() as libc::c_int;
        // SAFETY: the ioctl reads and writes one int.
        if unsafe { libc::ioctl(file.as_raw_fd(), WDIOC_SETTIMEOUT, &mut secs) } < 0 {
            warn!("setting the watchdog timeout: {}", std::io::Error::last_os_error());
        }
        info!("watchdog armed, {secs}s timeout");
        Some(Self { file, last_ping: Instant::now() })
    }

    pub fn next_ping(&self) -> Instant {
        self.last_ping + PING_EVERY
    }

    pub fn ping_if_due(&mut self, now: Instant) {
        if now < self.next_ping() {
            return;
        }
        if let Err(e) = self.file.write_all(b"\0") {
            warn!("pinging the watchdog: {e}");
        }
        self.last_ping = now;
    }

    /// The magic close: stop the watchdog on purpose, right before reboot.
    pub fn disarm(mut self) {
        let _ = self.file.write_all(b"V");
    }
}
