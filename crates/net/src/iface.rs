//! Interface configuration through the classic socket ioctls.
//!
//! Enough for one address and a default route. Bridges and taps for boxes need
//! rtnetlink and will move this over to it.

use std::ffi::CString;
use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// Wired interfaces backed by a device, sorted by name. Loopback and virtual
/// interfaces have no `device` link.
pub fn physical() -> Vec<String> {
    let Ok(dir) = fs::read_dir("/sys/class/net") else { return Vec::new() };
    let mut names: Vec<String> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("device").exists() && !e.path().join("wireless").exists())
        .filter(|e| fs::read_to_string(e.path().join("type")).is_ok_and(|t| t.trim() == "1"))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

pub fn mac(name: &str) -> io::Result<[u8; 6]> {
    let text = fs::read_to_string(format!("/sys/class/net/{name}/address"))?;
    let mut mac = [0u8; 6];
    let mut parts = text.trim().split(':');
    for b in &mut mac {
        *b = parts
            .next()
            .and_then(|p| u8::from_str_radix(p, 16).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("bad MAC {text:?}")))?;
    }
    Ok(mac)
}

pub fn has_carrier(name: &str) -> bool {
    fs::read_to_string(format!("/sys/class/net/{name}/carrier")).is_ok_and(|c| c.trim() == "1")
}

fn socket() -> io::Result<OwnedFd> {
    // SAFETY: plain syscall; the fd is owned from here on.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a fresh, valid descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn ifreq(name: &str) -> io::Result<libc::ifreq> {
    if name.len() >= libc::IFNAMSIZ {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "interface name too long"));
    }
    // SAFETY: ifreq is plain data; all zeroes is a valid value.
    let mut req: libc::ifreq = unsafe { std::mem::zeroed() };
    for (dst, src) in req.ifr_name.iter_mut().zip(name.bytes()) {
        *dst = src as libc::c_char;
    }
    Ok(req)
}

fn ioctl(fd: &OwnedFd, request: libc::Ioctl, arg: *mut libc::c_void) -> io::Result<()> {
    // SAFETY: callers pass a pointer to the struct the request expects.
    if unsafe { libc::ioctl(fd.as_raw_fd(), request, arg) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn sockaddr(ip: Ipv4Addr) -> libc::sockaddr {
    let sin = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr { s_addr: u32::from_ne_bytes(ip.octets()) },
        sin_zero: [0; 8],
    };
    // SAFETY: sockaddr_in and sockaddr are the same size and sockaddr is plain data.
    unsafe { std::mem::transmute(sin) }
}

pub fn set_up(name: &str) -> io::Result<()> {
    let fd = socket()?;
    let mut req = ifreq(name)?;
    ioctl(&fd, libc::SIOCGIFFLAGS, (&raw mut req).cast())?;
    // SAFETY: SIOCGIFFLAGS filled the flags member.
    unsafe { req.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short };
    ioctl(&fd, libc::SIOCSIFFLAGS, (&raw mut req).cast())
}

/// Sets the interface's address and netmask, replacing any previous one.
pub fn set_address(name: &str, ip: Ipv4Addr, netmask: Ipv4Addr) -> io::Result<()> {
    let fd = socket()?;
    let mut req = ifreq(name)?;
    req.ifr_ifru.ifru_addr = sockaddr(ip);
    ioctl(&fd, libc::SIOCSIFADDR, (&raw mut req).cast())?;
    req.ifr_ifru.ifru_netmask = sockaddr(netmask);
    ioctl(&fd, libc::SIOCSIFNETMASK, (&raw mut req).cast())
}

/// Removes the address, which also drops routes through it.
pub fn clear_address(name: &str) -> io::Result<()> {
    set_address(name, Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED)
}

pub fn add_default_route(name: &str, gateway: Ipv4Addr) -> io::Result<()> {
    let fd = socket()?;
    let dev = CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in name"))?;
    // SAFETY: rtentry is plain data; all zeroes is a valid value.
    let mut rt: libc::rtentry = unsafe { std::mem::zeroed() };
    rt.rt_dst = sockaddr(Ipv4Addr::UNSPECIFIED);
    rt.rt_genmask = sockaddr(Ipv4Addr::UNSPECIFIED);
    rt.rt_gateway = sockaddr(gateway);
    rt.rt_flags = libc::RTF_UP | libc::RTF_GATEWAY;
    rt.rt_dev = dev.as_ptr().cast_mut();
    match ioctl(&fd, libc::SIOCADDRT, (&raw mut rt).cast()) {
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => Ok(()),
        r => r,
    }
}

pub fn netmask_prefix(mask: Ipv4Addr) -> u32 {
    u32::from(mask).leading_ones()
}
