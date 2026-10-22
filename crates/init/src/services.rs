//! Starting services and keeping them running.
//!
//! The service list is compiled in: the image ships exactly these, and nothing
//! adds to it at run time. Each service runs in its own session and its own
//! cgroup under `system/`, with stdout and stderr on the kernel log.

use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use neslina_common::{error, info, warn};

use crate::mounts::SYSTEM_CGROUP;

pub struct Service {
    pub name: &'static str,
    pub path: &'static str,
    pub args: &'static [&'static str],
    pub restart: Restart,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    /// Long-running: restarted whenever it exits.
    Always,
    /// Runs once per boot: restarted only if it fails.
    OnFailure,
}

pub const SERVICES: &[Service] = &[
    Service { name: "net", path: "/usr/lib/neslina/neslina-net", args: &[], restart: Restart::Always },
    Service { name: "boot", path: "/usr/lib/neslina/neslina-boot", args: &[], restart: Restart::OnFailure },
    Service { name: "disks", path: "/usr/lib/neslina/neslina-disks", args: &[], restart: Restart::OnFailure },
];

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A service that ran this long before exiting is restarted at the minimum
/// backoff again, rather than treated as crash-looping.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

enum State {
    /// Waiting to be started at this time.
    Due(Instant),
    Running {
        pid: libc::pid_t,
        since: Instant,
    },
    /// Finished, or shutting down; never restarted.
    Stopped,
}

struct Entry {
    service: &'static Service,
    state: State,
    backoff: Duration,
}

pub struct Supervisor {
    entries: Vec<Entry>,
}

impl Supervisor {
    pub fn new(services: &'static [Service]) -> Self {
        let now = Instant::now();
        let entries =
            services.iter().map(|service| Entry { service, state: State::Due(now), backoff: BACKOFF_MIN }).collect();
        Self { entries }
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.entries
            .iter()
            .filter_map(|e| match e.state {
                State::Due(t) => Some(t),
                _ => None,
            })
            .min()
    }

    pub fn start_due(&mut self, now: Instant) {
        for e in &mut self.entries {
            let State::Due(at) = e.state else { continue };
            if at > now {
                continue;
            }
            match spawn(e.service) {
                Ok(pid) => {
                    info!("started {} (pid {pid})", e.service.name);
                    e.state = State::Running { pid, since: now };
                }
                Err(err) => {
                    error!("starting {}: {err}", e.service.name);
                    e.state = State::Due(now + e.backoff);
                    e.backoff = (e.backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }

    /// Reaps every exited child. Services are scheduled for restart; anything
    /// else is an orphan that was reparented to us.
    pub fn reap(&mut self, now: Instant) {
        loop {
            let mut status = 0;
            // SAFETY: status is a valid out pointer.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                return;
            }
            let Some(e) =
                self.entries.iter_mut().find(|e| matches!(e.state, State::Running { pid: p, .. } if p == pid))
            else {
                continue;
            };
            let State::Running { since, .. } = e.state else { unreachable!() };
            if e.service.restart == Restart::OnFailure && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                info!("{} finished", e.service.name);
                e.state = State::Stopped;
                continue;
            }
            if now.duration_since(since) >= HEALTHY_RUN {
                e.backoff = BACKOFF_MIN;
            }
            warn!("{} exited ({}), restarting in {}s", e.service.name, describe(status), e.backoff.as_secs());
            e.state = State::Due(now + e.backoff);
            e.backoff = (e.backoff * 2).min(BACKOFF_MAX);
        }
    }

    /// Asks every running service to stop and stops restarting them.
    pub fn stop_all(&mut self) {
        for e in &mut self.entries {
            if let State::Running { pid, .. } = e.state {
                // SAFETY: plain syscall; the negative pid is the service's process group.
                unsafe { libc::kill(-pid, libc::SIGTERM) };
            }
            e.state = State::Stopped;
        }
    }
}

fn spawn(service: &Service) -> std::io::Result<libc::pid_t> {
    let cgroup = format!("{SYSTEM_CGROUP}/{}", service.name);
    fs::create_dir_all(&cgroup)?;
    // Opened here, written in the child: the child moves itself into its cgroup
    // before exec, so nothing it starts ever runs outside it.
    let procs = OpenOptions::new().write(true).open(format!("{cgroup}/cgroup.procs"))?;
    let procs_fd = procs.as_raw_fd();
    let log = || -> std::io::Result<Stdio> { OpenOptions::new().write(true).open("/dev/kmsg").map(File::into) };

    let mut cmd = Command::new(service.path);
    cmd.args(service.args).env_clear().env("PATH", "/usr/bin").stdin(Stdio::null()).stdout(log()?).stderr(log()?);
    // SAFETY: only async-signal-safe syscalls run between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::write(procs_fd, b"0".as_ptr().cast(), 1) != 1 {
                return Err(std::io::Error::last_os_error());
            }
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    drop(procs);
    // The child is reaped by `reap` through waitpid(-1); the handle is not needed.
    Ok(child.id() as libc::pid_t)
}

fn describe(status: libc::c_int) -> String {
    if libc::WIFEXITED(status) {
        format!("status {}", libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        format!("signal {}", libc::WTERMSIG(status))
    } else {
        format!("wait status {status}")
    }
}
