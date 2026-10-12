//! UEFI variables through efivarfs.
//!
//! Each variable is a file named `<Name>-<vendor GUID>` whose contents are
//! four bytes of attributes followed by the value. The kernel marks variables
//! the firmware cares about (`Boot####`, `BootOrder`) immutable, so changing or
//! deleting one starts by clearing that flag.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

pub const DIR: &str = "/sys/firmware/efi/efivars";
/// EFI_GLOBAL_VARIABLE: Boot####, BootOrder, BootNext, BootCurrent.
pub const GLOBAL: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
/// neslina's own variables.
pub const NESLINA: &str = "1719486b-5d4c-4f12-8437-1fd3db3e03ac";

/// Non-volatile, boot-service and runtime access.
const NV_BS_RT: u32 = 0x7;

/// `_IOR('f', 1, long)` and `_IOW('f', 2, long)` from linux/fs.h.
const FS_IOC_GETFLAGS: libc::c_ulong = 0x8008_6601;
const FS_IOC_SETFLAGS: libc::c_ulong = 0x4008_6602;
const FS_IMMUTABLE_FL: u64 = 0x10;

fn path(name: &str, guid: &str) -> PathBuf {
    PathBuf::from(DIR).join(format!("{name}-{guid}"))
}

/// The variable's value without its attributes, or `None` if it does not exist.
pub fn read(name: &str, guid: &str) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path(name, guid)) {
        Ok(b) if b.len() >= 4 => Ok(Some(b[4..].to_vec())),
        Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{name} is shorter than its attributes"))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Creates or replaces a non-volatile variable.
pub fn write(name: &str, guid: &str, value: &[u8]) -> io::Result<()> {
    let p = path(name, guid);
    if p.exists() {
        set_immutable(&File::open(&p)?, false)?;
    }
    let mut buf = NV_BS_RT.to_le_bytes().to_vec();
    buf.extend_from_slice(value);
    // efivarfs takes the whole variable in one write, which replaces it;
    // truncating is neither needed nor supported.
    #[allow(clippy::suspicious_open_options)]
    let mut f = OpenOptions::new().write(true).create(true).open(&p)?;
    let n = f.write(&buf)?;
    if n != buf.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short write to {name}")));
    }
    Ok(())
}

pub fn delete(name: &str, guid: &str) -> io::Result<()> {
    let p = path(name, guid);
    match File::open(&p) {
        Ok(f) => set_immutable(&f, false)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    fs::remove_file(p)
}

fn set_immutable(f: &File, on: bool) -> io::Result<()> {
    let mut flags: u64 = 0;
    // SAFETY: both ioctls read or write the flags word we pass.
    if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_GETFLAGS, &mut flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let want = if on { flags | FS_IMMUTABLE_FL } else { flags & !FS_IMMUTABLE_FL };
    if want == flags {
        return Ok(());
    }
    let mut want = want;
    if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_SETFLAGS, &mut want) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn read_u16(name: &str) -> io::Result<Option<u16>> {
    Ok(read(name, GLOBAL)?.and_then(|v| Some(u16::from_le_bytes(v.get(..2)?.try_into().ok()?))))
}

pub fn read_u16s(name: &str) -> io::Result<Vec<u16>> {
    Ok(read(name, GLOBAL)?
        .map(|v| v.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect())
        .unwrap_or_default())
}

pub fn write_u16s(name: &str, values: &[u16]) -> io::Result<()> {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    write(name, GLOBAL, &bytes)
}

pub fn boot_name(number: u16) -> String {
    format!("Boot{number:04X}")
}

/// Numbers of every `Boot####` variable.
pub fn boot_entries() -> io::Result<Vec<u16>> {
    let suffix = format!("-{GLOBAL}");
    let mut out = Vec::new();
    for entry in fs::read_dir(DIR)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str().and_then(|n| n.strip_suffix(&suffix)) else { continue };
        if let Some(hex) = name.strip_prefix("Boot")
            && hex.len() == 4
            && let Ok(n) = u16::from_str_radix(hex, 16)
        {
            out.push(n);
        }
    }
    out.sort();
    Ok(out)
}
