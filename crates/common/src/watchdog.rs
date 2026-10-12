//! The hardware watchdog. If the system hangs, the board resets, and a slot
//! on trial falls back to the previous one.
//!
//! Stage 1 arms it first, with a long timeout that covers copying the root
//! image. Exec closes the device without the magic character, which leaves it
//! running, and init opens it again with its own timeout and keeps it fed.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use crate::{info, warn};

const DEVICE: &str = "/dev/watchdog0";
/// `_IOWR('W', 6, int)` from linux/watchdog.h.
const WDIOC_SETTIMEOUT: libc::c_ulong = 0xC004_5706;

pub struct Watchdog {
    file: File,
    last_ping: Instant,
    ping_every: Duration,
}

impl Watchdog {
    /// Arms the watchdog if the machine has one. It resets the board if
    /// nothing pings it for `timeout`.
    pub fn open(timeout: Duration) -> Option<Self> {
        let file = match OpenOptions::new().write(true).open(DEVICE) {
            Ok(f) => f,
            Err(e) => {
                warn!("no watchdog ({DEVICE}: {e}); a hang will need a manual reset");
                return None;
            }
        };
        let mut secs = timeout.as_secs() as libc::c_int;
        // SAFETY: the ioctl reads and writes one int.
        if unsafe { libc::ioctl(file.as_raw_fd(), WDIOC_SETTIMEOUT, &mut secs) } < 0 {
            warn!("setting the watchdog timeout: {}", std::io::Error::last_os_error());
        }
        info!("watchdog armed, {secs}s timeout");
        let mut w = Self { file, last_ping: Instant::now(), ping_every: Duration::from_secs(secs.max(3) as u64 / 3) };
        w.ping();
        Some(w)
    }

    pub fn next_ping(&self) -> Instant {
        self.last_ping + self.ping_every
    }

    pub fn ping_if_due(&mut self, now: Instant) {
        if now >= self.next_ping() {
            self.ping();
        }
    }

    pub fn ping(&mut self) {
        if let Err(e) = self.file.write_all(b"\0") {
            warn!("pinging the watchdog: {e}");
        }
        self.last_ping = Instant::now();
    }

    /// The magic close: stop the watchdog on purpose, right before reboot.
    pub fn disarm(mut self) {
        let _ = self.file.write_all(b"V");
    }
}
