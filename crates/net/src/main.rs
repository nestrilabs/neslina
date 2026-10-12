//! The host's network: loopback, then DHCP on the first wired interface with a
//! link. neslina has no static configuration; the network's DHCP server
//! decides the address.
//!
//! The hostname is `neslina-` and the last four hex digits of the interface's
//! MAC. It is set locally and sent to the DHCP server, so the host shows up in
//! the router's client list.

mod dhcp;
mod iface;

use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use neslina_common::{debug, info, sys, warn};

use dhcp::{MessageType, Reply, Request};

const RESOLV_CONF: &str = "/run/neslina/resolv.conf";
/// Lease length when the server sends none.
const DEFAULT_LEASE: Duration = Duration::from_secs(3600);
const RETRY_MIN: Duration = Duration::from_secs(4);
const RETRY_MAX: Duration = Duration::from_secs(64);

struct Lease {
    ip: Ipv4Addr,
    netmask: Ipv4Addr,
    router: Option<Ipv4Addr>,
    dns: Vec<Ipv4Addr>,
    domain: Option<String>,
    obtained: Instant,
    renew_after: Duration,
    expires_after: Duration,
}

fn main() {
    neslina_common::kmsg::init("net");
    if let Err(e) = run() {
        // init restarts us with backoff.
        neslina_common::error!("{e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    iface::set_up("lo").context("bringing up lo")?;

    let name = wait_for_link()?;
    let mac = iface::mac(&name)?;
    let hostname = format!("neslina-{:02x}{:02x}", mac[4], mac[5]);
    sys::sethostname(&hostname).context("setting the hostname")?;

    let sock = dhcp_socket(&name)?;
    let mut client = Client { sock, name, mac, hostname, xid: seed() };
    loop {
        let lease = client.acquire()?;
        client.apply(&lease)?;
        client.keep(lease)?;
    }
}

/// Brings every wired interface up and waits until one has a link.
fn wait_for_link() -> Result<String> {
    let mut reported = false;
    loop {
        let names = iface::physical();
        for n in &names {
            if let Err(e) = iface::set_up(n) {
                warn!("bringing up {n}: {e}");
            }
        }
        if let Some(n) = names.iter().find(|n| iface::has_carrier(n)) {
            info!("{n} has a link");
            return Ok(n.clone());
        }
        if !reported {
            info!(
                "waiting for a wired link on {}",
                if names.is_empty() { "any interface".into() } else { names.join(", ") }
            );
            reported = true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn dhcp_socket(name: &str) -> Result<UdpSocket> {
    let sock = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 68)).context("binding UDP port 68")?;
    sock.set_broadcast(true)?;
    // SAFETY: the option value is the interface name and its length.
    let ret = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.len() as libc::socklen_t,
        )
    };
    if ret < 0 {
        bail!("binding the DHCP socket to {name}: {}", std::io::Error::last_os_error());
    }
    Ok(sock)
}

/// A transaction ID seed that differs between hosts and boots.
fn seed() -> u32 {
    let mut b = [0u8; 4];
    // SAFETY: getrandom fills at most b.len() bytes.
    unsafe { libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0) };
    u32::from_ne_bytes(b)
}

struct Client {
    sock: UdpSocket,
    name: String,
    mac: [u8; 6],
    hostname: String,
    xid: u32,
}

impl Client {
    fn next_xid(&mut self) -> u32 {
        self.xid = self.xid.wrapping_add(1);
        self.xid
    }

    /// DISCOVER, OFFER, REQUEST, ACK. Retries forever with backoff: a host
    /// without an address has nothing better to do.
    fn acquire(&mut self) -> Result<Lease> {
        let mut wait = RETRY_MIN;
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            if let Some(lease) = self.try_acquire(wait)? {
                return Ok(lease);
            }
            if attempts == 3 {
                warn!("{}: no DHCP lease yet, still trying", self.name);
            }
            wait = (wait * 2).min(RETRY_MAX);
        }
    }

    fn try_acquire(&mut self, wait: Duration) -> Result<Option<Lease>> {
        let xid = self.next_xid();
        self.send(MessageType::Discover, xid, Ipv4Addr::UNSPECIFIED, None, None)?;
        let Some(offer) = self.receive(xid, wait, &[MessageType::Offer])? else {
            debug!("{}: no offer within {}s", self.name, wait.as_secs());
            return Ok(None);
        };
        self.send(MessageType::Request, xid, Ipv4Addr::UNSPECIFIED, Some(offer.yiaddr), offer.server)?;
        match self.receive(xid, wait, &[MessageType::Ack, MessageType::Nak])? {
            Some(ack) if ack.kind == MessageType::Ack => Ok(Some(lease_from(ack))),
            Some(_) => {
                warn!("{}: server refused {}", self.name, offer.yiaddr);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    /// Renews at T1 and keeps renewing until the lease expires. Returns when
    /// it has expired, after removing the address.
    fn keep(&mut self, mut lease: Lease) -> Result<()> {
        loop {
            let renew_at = lease.obtained + lease.renew_after;
            let expires = lease.obtained + lease.expires_after;
            std::thread::sleep(renew_at.saturating_duration_since(Instant::now()));

            while Instant::now() < expires {
                let xid = self.next_xid();
                self.send(MessageType::Request, xid, lease.ip, None, None)?;
                let left = expires.saturating_duration_since(Instant::now());
                match self.receive(xid, RETRY_MIN.min(left), &[MessageType::Ack, MessageType::Nak])? {
                    Some(ack) if ack.kind == MessageType::Ack => {
                        let new = lease_from(ack);
                        if new.ip != lease.ip || new.netmask != lease.netmask || new.router != lease.router {
                            self.apply(&new)?;
                        } else {
                            debug!("{}: renewed {}", self.name, new.ip);
                        }
                        lease = new;
                        break;
                    }
                    Some(_) => {
                        warn!("{}: server withdrew {}", self.name, lease.ip);
                        return self.drop_lease();
                    }
                    // Back off between tries, up to a minute, as RFC 2131 suggests.
                    None => std::thread::sleep((left / 2).clamp(Duration::ZERO, Duration::from_secs(60))),
                }
            }
            if Instant::now() >= expires {
                warn!("{}: lease on {} expired", self.name, lease.ip);
                return self.drop_lease();
            }
        }
    }

    fn drop_lease(&self) -> Result<()> {
        iface::clear_address(&self.name).with_context(|| format!("clearing {}'s address", self.name))
    }

    fn apply(&self, lease: &Lease) -> Result<()> {
        iface::set_address(&self.name, lease.ip, lease.netmask)
            .with_context(|| format!("setting {}'s address", self.name))?;
        if let Some(gw) = lease.router {
            iface::add_default_route(&self.name, gw).with_context(|| format!("adding a route via {gw}"))?;
        }
        write_resolv_conf(lease)?;
        info!(
            "{}: {}/{} via {}, dns {}, lease {}s, hostname {}",
            self.name,
            lease.ip,
            iface::netmask_prefix(lease.netmask),
            lease.router.map_or("none".into(), |r| r.to_string()),
            if lease.dns.is_empty() {
                "none".into()
            } else {
                lease.dns.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(" ")
            },
            lease.expires_after.as_secs(),
            self.hostname,
        );
        Ok(())
    }

    fn send(
        &self,
        kind: MessageType,
        xid: u32,
        ciaddr: Ipv4Addr,
        requested: Option<Ipv4Addr>,
        server: Option<Ipv4Addr>,
    ) -> Result<()> {
        let msg = Request { kind, xid, mac: self.mac, hostname: &self.hostname, ciaddr, requested, server }.encode();
        self.sock
            .send_to(&msg, SocketAddrV4::new(Ipv4Addr::BROADCAST, 67))
            .with_context(|| format!("sending DHCP {kind:?}"))?;
        Ok(())
    }

    /// Waits up to `wait` for a reply to `xid` of one of `kinds`. Anything else
    /// on the port (other clients' traffic, stale replies) is skipped.
    fn receive(&self, xid: u32, wait: Duration, kinds: &[MessageType]) -> Result<Option<Reply>> {
        let deadline = Instant::now() + wait;
        let mut buf = [0u8; 1500];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.sock.set_read_timeout(Some(left))?;
            let n = match self.sock.recv(&mut buf) {
                Ok(n) => n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Ok(None);
                }
                Err(e) => return Err(e).context("receiving DHCP"),
            };
            if let Some(r) = Reply::decode(&buf[..n])
                && r.xid == xid
                && r.mac == self.mac
                && kinds.contains(&r.kind)
            {
                return Ok(Some(r));
            }
        }
    }
}

fn lease_from(ack: Reply) -> Lease {
    let expires_after = ack.lease.unwrap_or(DEFAULT_LEASE);
    Lease {
        ip: ack.yiaddr,
        netmask: ack.netmask.unwrap_or(Ipv4Addr::new(255, 255, 255, 0)),
        router: ack.router,
        dns: ack.dns,
        domain: ack.domain,
        obtained: Instant::now(),
        renew_after: ack.renewal.unwrap_or(expires_after / 2).min(expires_after),
        expires_after,
    }
}

/// `/etc/resolv.conf` on the read-only root is a link to this file.
fn write_resolv_conf(lease: &Lease) -> Result<()> {
    let mut text = String::new();
    if let Some(d) = &lease.domain {
        text.push_str(&format!("search {d}\n"));
    }
    for ns in &lease.dns {
        text.push_str(&format!("nameserver {ns}\n"));
    }
    let tmp = format!("{RESOLV_CONF}.tmp");
    fs::write(&tmp, text).with_context(|| format!("writing {tmp}"))?;
    fs::rename(&tmp, RESOLV_CONF).with_context(|| format!("replacing {RESOLV_CONF}"))?;
    Ok(())
}
