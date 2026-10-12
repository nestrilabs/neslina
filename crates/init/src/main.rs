//! PID 1 of a neslina host.
//!
//! It mounts what the read-only root needs, sets up the cgroup tree, starts the
//! services and restarts them when they exit, reaps orphans, keeps the hardware
//! watchdog fed, and shuts down in order. Nothing else: everything that can be
//! a service is one, because a panic here is a kernel panic.
//!
//! Signals, as other small inits use them:
//! - SIGTERM, or SIGINT (Ctrl-Alt-Del): reboot
//! - SIGUSR2: power off

mod mounts;
mod services;

use std::time::{Duration, Instant};

use neslina_common::{error, info};

use neslina_common::watchdog::Watchdog;
use services::{SERVICES, Supervisor};

/// A hang this long resets the board.
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);
/// How long services get to exit on SIGTERM before everything is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug)]
enum Shutdown {
    Reboot,
    PowerOff,
}

fn main() {
    neslina_common::kmsg::init("init");
    if std::process::id() != 1 {
        error!("must run as PID 1");
        std::process::exit(1);
    }

    if let Err(e) = mounts::setup() {
        // Without its filesystems nothing can run; rebooting falls back to the previous slot.
        error!("{e:#}");
        shutdown(Shutdown::Reboot, &mut Supervisor::new(&[]), &mut None);
    }

    // Ctrl-Alt-Del becomes SIGINT to us instead of an immediate reboot.
    // SAFETY: plain syscall with constant arguments.
    unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_CAD_OFF) };

    let signals = match Signals::block() {
        Ok(s) => s,
        Err(e) => {
            error!("setting up signals: {e}");
            shutdown(Shutdown::Reboot, &mut Supervisor::new(&[]), &mut None);
        }
    };

    let mut watchdog = Watchdog::open(WATCHDOG_TIMEOUT);
    let mut sup = Supervisor::new(SERVICES);
    sup.start_due(Instant::now());
    info!("up");

    loop {
        let now = Instant::now();
        let mut wake = now + Duration::from_secs(60);
        if let Some(t) = sup.next_due() {
            wake = wake.min(t);
        }
        if let Some(w) = watchdog.as_mut() {
            wake = wake.min(w.next_ping());
        }

        for sig in signals.wait(wake.saturating_duration_since(now)) {
            match sig {
                libc::SIGCHLD => sup.reap(Instant::now()),
                libc::SIGTERM | libc::SIGINT => shutdown(Shutdown::Reboot, &mut sup, &mut watchdog),
                libc::SIGUSR2 => shutdown(Shutdown::PowerOff, &mut sup, &mut watchdog),
                _ => {}
            }
        }

        let now = Instant::now();
        if let Some(w) = watchdog.as_mut() {
            w.ping_if_due(now);
        }
        sup.start_due(now);
    }
}

fn shutdown(how: Shutdown, sup: &mut Supervisor, watchdog: &mut Option<Watchdog>) -> ! {
    info!("shutting down: {how:?}");
    sup.stop_all();
    // SAFETY: kill(-1) signals every process except PID 1; plain syscall.
    unsafe { libc::kill(-1, libc::SIGTERM) };
    let deadline = Instant::now() + STOP_GRACE;
    while !no_children() && Instant::now() < deadline {
        if let Some(w) = watchdog.as_mut() {
            w.ping_if_due(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !no_children() {
        info!("killing what is left after {}s", STOP_GRACE.as_secs());
        // SAFETY: as above.
        unsafe { libc::kill(-1, libc::SIGKILL) };
        while !no_children() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    mounts::teardown();
    if let Some(w) = watchdog.take() {
        w.disarm();
    }
    let cmd = match how {
        Shutdown::Reboot => libc::LINUX_REBOOT_CMD_RESTART,
        Shutdown::PowerOff => libc::LINUX_REBOOT_CMD_POWER_OFF,
    };
    // SAFETY: plain syscall; does not return on success.
    unsafe { libc::reboot(cmd) };
    error!("reboot failed: {}", std::io::Error::last_os_error());
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Reaps everything that has exited and reports whether any child is left.
fn no_children() -> bool {
    loop {
        // SAFETY: waitpid with a null status pointer is allowed.
        let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if pid > 0 {
            continue;
        }
        return pid < 0; // ECHILD: nothing left. 0: children still running.
    }
}

/// The signals init handles, blocked and read from a signalfd so they arrive
/// in the main loop instead of in a handler.
struct Signals {
    fd: libc::c_int,
}

impl Signals {
    const HANDLED: [libc::c_int; 4] = [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT, libc::SIGUSR2];

    fn block() -> std::io::Result<Self> {
        // SAFETY: sigset_t is plain data, initialised by sigemptyset before use.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for s in Self::HANDLED {
                libc::sigaddset(&mut set, s);
            }
            if libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self { fd })
        }
    }

    /// Waits up to `timeout` and returns the signals that arrived, if any.
    fn wait(&self, timeout: Duration) -> Vec<libc::c_int> {
        let mut pfd = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        // SAFETY: one valid pollfd.
        unsafe { libc::poll(&mut pfd, 1, ms) };

        let mut out = Vec::new();
        loop {
            // SAFETY: signalfd_siginfo is plain data; read fills at most its size.
            let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::signalfd_siginfo>();
            let n = unsafe { libc::read(self.fd, (&raw mut info).cast(), size) };
            if n as usize != size {
                break;
            }
            out.push(info.ssi_signo as libc::c_int);
        }
        out
    }
}
