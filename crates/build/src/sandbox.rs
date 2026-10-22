//! Running a build script inside the sysroot.
//!
//! Each run gets new user, mount, PID, network, IPC and UTS namespaces. The
//! sysroot's directories are bound in read-only, plus the binds the caller
//! names. There is no network, and no host path is visible except those
//! binds and a few device nodes. Inside, the script runs as root of its own
//! user namespace, which is one unprivileged uid outside:
//! - run by an ordinary user (a dev machine), that user;
//! - run by root (a host), `BUILD_UID`, so a build never runs as real root.
//!
//! The same code works in both cases because it needs nothing but user
//! namespaces.
//!
//! `run` forks. It must be called while the process is single-threaded.

use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The uid and gid builds run as on a host, where neslina runs as root.
pub const BUILD_UID: u32 = 64_000;

/// Sysroot top-level directories that are not bound in; the sandbox makes its
/// own where it needs one.
const SKIP: &[&str] = &["dev", "proc", "sys", "tmp", "run", "home", "root", "mnt", "boot", "srv", "lost+found"];
const DEVICES: &[&str] = &["null", "zero", "full", "random", "urandom", "tty"];

pub struct Bind {
    pub host: PathBuf,
    pub inside: PathBuf,
    pub writable: bool,
}

impl Bind {
    pub fn ro(host: impl Into<PathBuf>, inside: impl Into<PathBuf>) -> Self {
        Self { host: host.into(), inside: inside.into(), writable: false }
    }

    pub fn rw(host: impl Into<PathBuf>, inside: impl Into<PathBuf>) -> Self {
        Self { host: host.into(), inside: inside.into(), writable: true }
    }
}

pub struct Sandbox<'a> {
    pub sysroot: &'a Path,
    /// An empty directory the sandbox's root is mounted over. Only the
    /// sandbox's own mount namespace sees anything there.
    pub scratch: PathBuf,
    pub binds: Vec<Bind>,
    /// Read-only merged views: the host directories, lowest first, shown as
    /// one at the inside path.
    pub overlays: Vec<(Vec<PathBuf>, PathBuf)>,
    pub env: Vec<(String, String)>,
    pub workdir: PathBuf,
}

/// The uid outside that root inside maps to.
pub fn outside_uid() -> u32 {
    // SAFETY: plain syscall.
    match unsafe { libc::geteuid() } {
        0 => BUILD_UID,
        uid => uid,
    }
}

fn outside_gid() -> u32 {
    // SAFETY: plain syscall.
    match unsafe { libc::geteuid() } {
        0 => BUILD_UID,
        // SAFETY: plain syscall.
        _ => unsafe { libc::getegid() },
    }
}

fn pipe() -> io::Result<(fs::File, fs::File)> {
    let mut fds = [0; 2];
    // SAFETY: fds has room for the two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fresh descriptors, owned from here on.
    unsafe { Ok((fs::File::from(OwnedFd::from_raw_fd(fds[0])), fs::File::from(OwnedFd::from_raw_fd(fds[1])))) }
}

fn check(ret: libc::c_int, what: &str) -> Result<()> {
    if ret < 0 {
        bail!("{what}: {}", io::Error::last_os_error());
    }
    Ok(())
}

fn wait(pid: libc::pid_t) -> io::Result<libc::c_int> {
    let mut status = 0;
    loop {
        // SAFETY: status is a valid out pointer.
        if unsafe { libc::waitpid(pid, &mut status, 0) } >= 0 {
            return Ok(status);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) }
}

impl Sandbox<'_> {
    /// Runs `script` with bash (`-euo pipefail`) and fails unless it exits 0.
    pub fn run(&self, script: &str) -> Result<()> {
        fs::create_dir_all(&self.scratch)?;
        let (mut to_parent_r, mut to_parent_w) = pipe()?;
        let (mut to_child_r, mut to_child_w) = pipe()?;

        // SAFETY: the caller guarantees a single-threaded process, so the
        // child may allocate and use std until it execs or exits.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            bail!("fork: {}", io::Error::last_os_error());
        }
        if pid == 0 {
            drop((to_parent_r, to_child_w));
            let code = match self.child(script, &mut to_parent_w, &mut to_child_r) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("sandbox: {e:#}");
                    125
                }
            };
            // SAFETY: leaving the forked child without running the parent's destructors.
            unsafe { libc::_exit(code) };
        }
        drop((to_parent_w, to_child_r));

        let mut byte = [0u8; 1];
        let mapped = to_parent_r
            .read_exact(&mut byte)
            .context("waiting for the sandbox to unshare")
            .and_then(|()| write_id_maps(pid))
            .and_then(|()| to_child_w.write_all(b"1").context("releasing the sandbox"));
        let status = wait(pid)?;
        mapped?;
        match exit_code(status) {
            0 => Ok(()),
            code => bail!("build script failed with exit code {code}"),
        }
    }

    /// Runs in the forked child; returns its exit code.
    fn child(&self, script: &str, to_parent: &mut fs::File, from_parent: &mut fs::File) -> Result<i32> {
        let flags = libc::CLONE_NEWUSER
            | libc::CLONE_NEWNS
            | libc::CLONE_NEWPID
            | libc::CLONE_NEWNET
            | libc::CLONE_NEWIPC
            | libc::CLONE_NEWUTS;
        // SAFETY: plain syscall.
        check(unsafe { libc::unshare(flags) }, "unshare")?;
        to_parent.write_all(b"1")?;
        let mut byte = [0u8; 1];
        from_parent.read_exact(&mut byte).context("waiting for the id maps")?;
        // Root inside. As root outside, the host uid is not in the map until
        // this switches to the mapped one.
        // SAFETY: plain syscalls; we hold every capability in the new namespace.
        unsafe {
            check(libc::setresgid(0, 0, 0), "setresgid")?;
            check(libc::setresuid(0, 0, 0), "setresuid")?;
        }

        // The new PID namespace applies to children; the script runs as its PID 1.
        // SAFETY: still single-threaded.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            bail!("fork: {}", io::Error::last_os_error());
        }
        if pid == 0 {
            let err = match self.enter() {
                Ok(()) => self.command(script).exec().into(),
                Err(e) => e,
            };
            eprintln!("sandbox: {err:#}");
            // SAFETY: as above.
            unsafe { libc::_exit(125) };
        }
        Ok(exit_code(wait(pid)?))
    }

    fn command(&self, script: &str) -> Command {
        let mut cmd = Command::new("/usr/bin/bash");
        cmd.args(["--noprofile", "--norc", "-euo", "pipefail", "-c", script])
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin")
            .env("HOME", "/tmp")
            .env("LANG", "C.UTF-8")
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&self.workdir);
        cmd
    }

    /// Builds the sandbox's filesystem and moves into it.
    fn enter(&self) -> Result<()> {
        mount("", "/", "", libc::MS_REC | libc::MS_PRIVATE, None)?;
        let root = self.scratch.as_path();
        mount("tmpfs", root, "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, Some("mode=0755"))?;

        for entry in fs::read_dir(self.sysroot).with_context(|| format!("reading {}", self.sysroot.display()))? {
            let entry = entry?;
            let name = entry.file_name();
            let (src, dst) = (entry.path(), root.join(&name));
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                symlink(fs::read_link(&src)?, &dst)?;
            } else if kind.is_dir() && !SKIP.contains(&name.to_str().unwrap_or_default()) {
                fs::create_dir(&dst)?;
                bind(&src, &dst, false)?;
            }
        }

        for dir in ["dev", "proc", "tmp", "run"] {
            fs::create_dir_all(root.join(dir))?;
        }
        mount("tmpfs", root.join("tmp"), "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, Some("mode=1777"))?;
        mount("tmpfs", root.join("dev"), "tmpfs", libc::MS_NOSUID | libc::MS_NOEXEC, Some("mode=0755"))?;
        for dev in DEVICES {
            let node = root.join("dev").join(dev);
            fs::File::create(&node)?;
            bind(&Path::new("/dev").join(dev), &node, true)?;
        }
        for (link, target) in [
            ("fd", "/proc/self/fd"),
            ("stdin", "/proc/self/fd/0"),
            ("stdout", "/proc/self/fd/1"),
            ("stderr", "/proc/self/fd/2"),
        ] {
            symlink(target, root.join("dev").join(link))?;
        }
        fs::create_dir(root.join("dev/shm"))?;
        mount("tmpfs", root.join("dev/shm"), "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, Some("mode=1777"))?;
        mount("proc", root.join("proc"), "proc", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None)?;

        for b in &self.binds {
            let dst = root.join(b.inside.strip_prefix("/").unwrap_or(&b.inside));
            if b.host.is_dir() {
                fs::create_dir_all(&dst)?;
            } else {
                fs::create_dir_all(dst.parent().unwrap_or(root))?;
                fs::File::create(&dst)?;
            }
            bind(&b.host, &dst, b.writable).with_context(|| format!("binding {}", b.host.display()))?;
        }

        for (layers, inside) in &self.overlays {
            let dst = root.join(inside.strip_prefix("/").unwrap_or(inside));
            fs::create_dir_all(&dst)?;
            match layers.as_slice() {
                [] => {}
                [one] => bind(one, &dst, false)?,
                many => {
                    // overlayfs lists the top layer first.
                    let lower: Vec<&str> = many.iter().rev().map(|p| p.to_str().unwrap_or_default()).collect();
                    mount("overlay", &dst, "overlay", libc::MS_RDONLY, Some(&format!("lowerdir={}", lower.join(":"))))?;
                }
            }
        }

        // Pivot onto the new root and drop the old one entirely.
        std::env::set_current_dir(root)?;
        let dot = CString::new(".")?;
        // SAFETY: both arguments are ".", a valid NUL-terminated string.
        check(unsafe { libc::syscall(libc::SYS_pivot_root, dot.as_ptr(), dot.as_ptr()) } as libc::c_int, "pivot_root")?;
        // SAFETY: as above.
        check(unsafe { libc::umount2(dot.as_ptr(), libc::MNT_DETACH) }, "detaching the old root")?;
        std::env::set_current_dir("/")?;
        let host = b"neslina-build";
        // SAFETY: pointer and length describe `host`.
        unsafe { libc::sethostname(host.as_ptr().cast(), host.len()) };
        Ok(())
    }
}

fn write_id_maps(pid: libc::pid_t) -> Result<()> {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    // Unprivileged gid maps need setgroups denied; it changes nothing for root.
    fs::write(proc.join("setgroups"), "deny").context("writing setgroups")?;
    fs::write(proc.join("uid_map"), format!("0 {} 1\n", outside_uid())).context("writing uid_map")?;
    fs::write(proc.join("gid_map"), format!("0 {} 1\n", outside_gid())).context("writing gid_map")?;
    Ok(())
}

fn mount(src: &str, target: impl AsRef<Path>, fstype: &str, flags: libc::c_ulong, data: Option<&str>) -> Result<()> {
    let target = target.as_ref();
    let c = |s: &str| CString::new(s).context("NUL in mount argument");
    let (src_c, fstype_c) = (c(src)?, c(fstype)?);
    let target_c = c(target.to_str().context("non-UTF-8 path")?)?;
    let data_c = data.map(c).transpose()?;
    // SAFETY: every pointer is a valid NUL-terminated string that outlives the call.
    let ret = unsafe {
        libc::mount(
            src_c.as_ptr(),
            target_c.as_ptr(),
            fstype_c.as_ptr(),
            flags,
            data_c.as_ref().map_or(std::ptr::null(), |d| d.as_ptr().cast()),
        )
    };
    check(ret, &format!("mounting {} on {}", if src.is_empty() { fstype } else { src }, target.display()))
}

/// Bind-mounts `src` on `dst`, read-only unless `writable`.
fn bind(src: &Path, dst: &Path, writable: bool) -> Result<()> {
    mount(src.to_str().context("non-UTF-8 path")?, dst, "", libc::MS_BIND | libc::MS_REC, None)?;
    if writable {
        return Ok(());
    }
    // A remount inside a user namespace must keep the flags the outer mount
    // locked (nosuid, nodev, noexec, the atime mode), or the kernel refuses it.
    let path = CString::new(dst.to_str().context("non-UTF-8 path")?)?;
    // SAFETY: statvfs is plain data, filled by the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid path and out pointer.
    check(unsafe { libc::statvfs(path.as_ptr(), &mut st) }, "statvfs")?;
    let mut flags = libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY;
    for (st_flag, ms_flag) in [
        (libc::ST_NOSUID, libc::MS_NOSUID),
        (libc::ST_NODEV, libc::MS_NODEV),
        (libc::ST_NOEXEC, libc::MS_NOEXEC),
        (libc::ST_NOATIME, libc::MS_NOATIME),
        (libc::ST_NODIRATIME, libc::MS_NODIRATIME),
        (libc::ST_RELATIME, libc::MS_RELATIME),
    ] {
        if st.f_flag & st_flag != 0 {
            flags |= ms_flag;
        }
    }
    mount("", dst, "", flags, None)
}
