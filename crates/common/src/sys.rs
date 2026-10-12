//! Thin wrappers over the few syscalls std does not cover.

use std::ffi::CString;
use std::io;
use std::path::Path;

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn cpath(p: &Path) -> io::Result<CString> {
    cstr(p.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not UTF-8"))?)
}

fn check(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// `mount(2)`. `source` and `fstype` may be empty for a move or remount.
pub fn mount(
    source: &str,
    target: impl AsRef<Path>,
    fstype: &str,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> io::Result<()> {
    let source = cstr(source)?;
    let target = cpath(target.as_ref())?;
    let fstype = cstr(fstype)?;
    let data = data.map(cstr).transpose()?;
    // SAFETY: every pointer is a valid NUL-terminated string that outlives the call.
    check(unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            flags,
            data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr().cast()),
        )
    })
}

pub fn umount(target: impl AsRef<Path>, flags: libc::c_int) -> io::Result<()> {
    let target = cpath(target.as_ref())?;
    // SAFETY: valid NUL-terminated string.
    check(unsafe { libc::umount2(target.as_ptr(), flags) })
}

pub fn chroot(path: impl AsRef<Path>) -> io::Result<()> {
    let path = cpath(path.as_ref())?;
    // SAFETY: valid NUL-terminated string.
    check(unsafe { libc::chroot(path.as_ptr()) })
}

pub fn sethostname(name: &str) -> io::Result<()> {
    // SAFETY: pointer and length describe `name`.
    check(unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) })
}

/// Whether `path` is the root of a mount: its device differs from its parent's.
pub fn is_mountpoint(path: impl AsRef<Path>) -> bool {
    use std::os::unix::fs::MetadataExt;
    let path = path.as_ref();
    let (Ok(me), Ok(parent)) = (std::fs::metadata(path), std::fs::metadata(path.join(".."))) else {
        return false;
    };
    me.dev() != parent.dev() || me.ino() == parent.ino()
}
