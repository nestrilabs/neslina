//! A minimal mDNS responder (RFC 6762): the host answers for `<hostname>.local`
//! with its IPv4 address, so the owner can reach the setup page by name
//! without knowing what the DHCP server handed out.
//!
//! It answers A (and ANY) questions for that one name and announces itself
//! when the address changes. Queries from ordinary resolvers, which come from
//! a port other than 5353, get a unicast reply that echoes the question, as
//! RFC 6762 §6.7 asks.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::fd::FromRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use neslina_common::{debug, warn};

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;
const TYPE_A: u16 = 1;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
/// Top bit of the class: in a question, "unicast reply wanted"; in an answer,
/// "flush older records for this name".
const CLASS_TOP_BIT: u16 = 0x8000;
const TTL: u32 = 120;

pub struct Responder {
    sock: Arc<UdpSocket>,
    name: String,
    addr: Arc<Mutex<Option<Ipv4Addr>>>,
}

impl Responder {
    /// Binds port 5353 and starts answering. Until `set_address`, there is
    /// nothing to answer with.
    pub fn start(hostname: &str) -> io::Result<Self> {
        let sock = Arc::new(bind()?);
        sock.set_multicast_ttl_v4(255)?;
        let name = format!("{hostname}.local");
        let addr = Arc::new(Mutex::new(None));
        let (s, n, a) = (sock.clone(), name.clone(), addr.clone());
        std::thread::Builder::new().name("mdns".into()).spawn(move || serve(&s, &n, &a))?;
        Ok(Self { sock, name, addr })
    }

    /// Joins the multicast group on the interface with this address, answers
    /// with it from now on, and announces it (twice, a second apart, §8.3).
    pub fn set_address(&self, ip: Ipv4Addr) {
        let previous = self.addr.lock().unwrap_or_else(|e| e.into_inner()).replace(ip);
        if previous == Some(ip) {
            return;
        }
        if let Some(old) = previous {
            let _ = self.sock.leave_multicast_v4(&GROUP, &old);
        }
        if let Err(e) = self.sock.join_multicast_v4(&GROUP, &ip) {
            warn!("mdns: joining {GROUP} on {ip}: {e}");
            return;
        }
        set_multicast_if(&self.sock, ip);
        let announcement = answer(&self.name, ip, None);
        for i in 0..2 {
            if i > 0 {
                std::thread::sleep(Duration::from_secs(1));
            }
            if let Err(e) = self.sock.send_to(&announcement, SocketAddrV4::new(GROUP, PORT)) {
                warn!("mdns: announcing: {e}");
            }
        }
    }
}

/// UDP 5353 with SO_REUSEADDR, so another responder on the host (there is
/// none today) would not stop this one.
fn bind() -> io::Result<UdpSocket> {
    // SAFETY: plain syscalls on a fresh descriptor, owned by the UdpSocket after.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let sock = UdpSocket::from_raw_fd(fd);
        let one: libc::c_int = 1;
        let size = std::mem::size_of_val(&one) as libc::socklen_t;
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, (&raw const one).cast(), size);
        let sin = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: PORT.to_be(),
            sin_addr: libc::in_addr { s_addr: 0 },
            sin_zero: [0; 8],
        };
        let len = std::mem::size_of_val(&sin) as libc::socklen_t;
        if libc::bind(fd, (&raw const sin).cast(), len) < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sock)
    }
}

fn set_multicast_if(sock: &UdpSocket, ip: Ipv4Addr) {
    use std::os::fd::AsRawFd;
    let addr = libc::in_addr { s_addr: u32::from_ne_bytes(ip.octets()) };
    let size = std::mem::size_of_val(&addr) as libc::socklen_t;
    // SAFETY: the option value is one in_addr.
    unsafe {
        libc::setsockopt(sock.as_raw_fd(), libc::IPPROTO_IP, libc::IP_MULTICAST_IF, (&raw const addr).cast(), size)
    };
}

fn serve(sock: &UdpSocket, name: &str, addr: &Mutex<Option<Ipv4Addr>>) {
    let mut buf = [0u8; 9000];
    loop {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) => {
                warn!("mdns: receiving: {e}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let Some(ip) = *addr.lock().unwrap_or_else(|e| e.into_inner()) else { continue };
        let Some(q) = Query::parse(&buf[..n]) else { continue };
        let Some(unicast) = q.asks_for(name) else { continue };
        let legacy = from.port() != PORT;
        let (reply, to) = if legacy {
            (answer(name, ip, Some(&q)), from)
        } else if unicast {
            (answer(name, ip, None), from)
        } else {
            (answer(name, ip, None), SocketAddr::V4(SocketAddrV4::new(GROUP, PORT)))
        };
        debug!("mdns: answering {from} for {name}");
        if let Err(e) = sock.send_to(&reply, to) {
            warn!("mdns: replying to {from}: {e}");
        }
    }
}

/// The parts of a query a responder for one name needs.
#[derive(Debug, PartialEq, Eq)]
struct Query {
    id: u16,
    /// Name, type and class of each question.
    questions: Vec<(String, u16, u16)>,
    /// The question section as received, for echoing in a legacy reply.
    raw_questions: Vec<u8>,
}

impl Query {
    fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 12 {
            return None;
        }
        let flags = u16::from_be_bytes([b[2], b[3]]);
        if flags & 0x8000 != 0 {
            return None; // a response, not a query
        }
        let count = u16::from_be_bytes([b[4], b[5]]);
        let mut at = 12;
        let mut questions = Vec::new();
        for _ in 0..count {
            let (name, next) = read_name(b, at)?;
            let fixed = b.get(next..next + 4)?;
            questions.push((name, u16::from_be_bytes([fixed[0], fixed[1]]), u16::from_be_bytes([fixed[2], fixed[3]])));
            at = next + 4;
        }
        Some(Query { id: u16::from_be_bytes([b[0], b[1]]), questions, raw_questions: b[12..at].to_vec() })
    }

    /// Whether any question is for `name` (A or ANY, class IN), and if so
    /// whether a unicast reply was asked for.
    fn asks_for(&self, name: &str) -> Option<bool> {
        self.questions
            .iter()
            .find(|(n, t, c)| {
                n.eq_ignore_ascii_case(name) && (*t == TYPE_A || *t == TYPE_ANY) && c & !CLASS_TOP_BIT == CLASS_IN
            })
            .map(|(_, _, c)| c & CLASS_TOP_BIT != 0)
    }
}

/// Reads a possibly compressed name starting at `at`; returns it and where
/// the name ends in the message.
fn read_name(b: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    for _ in 0..64 {
        let len = *b.get(at)? as usize;
        if len == 0 {
            return Some((labels.join("."), end.unwrap_or(at + 1)));
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *b.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            at = ptr;
            continue;
        }
        labels.push(String::from_utf8_lossy(b.get(at + 1..at + 1 + len)?).into_owned());
        at += 1 + len;
    }
    None // a pointer loop
}

fn write_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// An authoritative answer with our A record. For a legacy query it carries
/// the query's id and question, and no cache-flush bit (§6.7).
fn answer(name: &str, ip: Ipv4Addr, legacy: Option<&Query>) -> Vec<u8> {
    let mut b = Vec::with_capacity(64);
    b.extend_from_slice(&legacy.map_or(0, |q| q.id).to_be_bytes());
    b.extend_from_slice(&0x8400u16.to_be_bytes()); // response, authoritative
    let qd = legacy.map_or(0, |q| q.questions.len() as u16);
    for count in [qd, 1, 0, 0] {
        b.extend_from_slice(&count.to_be_bytes());
    }
    if let Some(q) = legacy {
        b.extend_from_slice(&q.raw_questions);
    }
    write_name(&mut b, name);
    b.extend_from_slice(&TYPE_A.to_be_bytes());
    let class = if legacy.is_some() { CLASS_IN } else { CLASS_IN | CLASS_TOP_BIT };
    b.extend_from_slice(&class.to_be_bytes());
    b.extend_from_slice(&TTL.to_be_bytes());
    b.extend_from_slice(&4u16.to_be_bytes());
    b.extend_from_slice(&ip.octets());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(id: u16, name: &str, qtype: u16, class: u16) -> Vec<u8> {
        let mut b = id.to_be_bytes().to_vec();
        b.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        write_name(&mut b, name);
        b.extend_from_slice(&qtype.to_be_bytes());
        b.extend_from_slice(&class.to_be_bytes());
        b
    }

    #[test]
    fn answers_only_its_own_name_and_type() {
        let name = "neslina-3456.local";
        let q = Query::parse(&query(0, "NESLINA-3456.local", TYPE_A, CLASS_IN)).unwrap();
        assert_eq!(q.asks_for(name), Some(false));
        let qu = Query::parse(&query(0, name, TYPE_ANY, CLASS_IN | CLASS_TOP_BIT)).unwrap();
        assert_eq!(qu.asks_for(name), Some(true));
        assert_eq!(Query::parse(&query(0, "other.local", TYPE_A, CLASS_IN)).unwrap().asks_for(name), None);
        assert_eq!(Query::parse(&query(0, name, 28, CLASS_IN)).unwrap().asks_for(name), None); // AAAA
    }

    #[test]
    fn ignores_responses_and_garbage() {
        let mut r = query(0, "x.local", TYPE_A, CLASS_IN);
        r[2] = 0x84;
        assert!(Query::parse(&r).is_none());
        assert!(Query::parse(&[0; 5]).is_none());
        let mut cut = query(0, "x.local", TYPE_A, CLASS_IN);
        cut.truncate(cut.len() - 3);
        assert!(Query::parse(&cut).is_none());
    }

    #[test]
    fn follows_compression_and_refuses_loops() {
        // Two questions; the second name is a pointer to the first (offset 12).
        let mut b = vec![0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0];
        write_name(&mut b, "neslina-3456.local");
        b.extend_from_slice(&[0, 28, 0, 1]);
        b.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1]);
        let q = Query::parse(&b).unwrap();
        assert_eq!(q.questions[1], ("neslina-3456.local".into(), TYPE_A, CLASS_IN));
        assert_eq!(q.asks_for("neslina-3456.local"), Some(false));

        let looped = [0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0, 12, 0, 1, 0, 1];
        assert!(Query::parse(&looped).is_none());
    }

    #[test]
    fn answer_layout() {
        let ip = Ipv4Addr::new(10, 0, 2, 15);
        let a = answer("h.local", ip, None);
        assert_eq!(&a[0..12], &[0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(&a[a.len() - 4..], &ip.octets());
        assert_eq!(&a[a.len() - 12..a.len() - 10], &(CLASS_IN | CLASS_TOP_BIT).to_be_bytes());

        let q = Query::parse(&query(0x1234, "h.local", TYPE_A, CLASS_IN)).unwrap();
        let legacy = answer("h.local", ip, Some(&q));
        assert_eq!(&legacy[0..2], &[0x12, 0x34]);
        assert_eq!(&legacy[4..6], &[0, 1]); // the question is echoed
        assert_eq!(&legacy[legacy.len() - 12..legacy.len() - 10], &CLASS_IN.to_be_bytes());
    }
}
