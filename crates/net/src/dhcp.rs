//! DHCPv4 messages (RFC 2131 and 2132), only what a client with one lease needs.

use std::net::Ipv4Addr;
use std::time::Duration;

const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
/// BOOTP's minimum message size; some servers drop anything shorter.
const MIN_LEN: usize = 300;
const FLAG_BROADCAST: u16 = 0x8000;

const OPT_PAD: u8 = 0;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_HOSTNAME: u8 = 12;
const OPT_DOMAIN: u8 = 15;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_PARAMS: u8 = 55;
const OPT_MAX_SIZE: u8 = 57;
const OPT_RENEWAL: u8 = 58;
const OPT_REBINDING: u8 = 59;
const OPT_CLIENT_ID: u8 = 61;
const OPT_END: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
}

impl MessageType {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Discover,
            2 => Self::Offer,
            3 => Self::Request,
            4 => Self::Decline,
            5 => Self::Ack,
            6 => Self::Nak,
            7 => Self::Release,
            _ => return None,
        })
    }
}

/// What the client sends.
pub struct Request<'a> {
    pub kind: MessageType,
    pub xid: u32,
    pub mac: [u8; 6],
    pub hostname: &'a str,
    /// Our current address, when renewing.
    pub ciaddr: Ipv4Addr,
    /// The offered address, when requesting from an offer.
    pub requested: Option<Ipv4Addr>,
    pub server: Option<Ipv4Addr>,
}

impl Request<'_> {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; 236];
        b[0] = 1; // BOOTREQUEST
        b[1] = 1; // Ethernet
        b[2] = 6;
        b[4..8].copy_from_slice(&self.xid.to_be_bytes());
        // Ask for broadcast replies: until it is configured, the interface has
        // no address for a unicast reply to reach.
        b[10..12].copy_from_slice(&FLAG_BROADCAST.to_be_bytes());
        b[12..16].copy_from_slice(&self.ciaddr.octets());
        b[28..34].copy_from_slice(&self.mac);
        b.extend_from_slice(&MAGIC_COOKIE);

        let mut opt = |code: u8, data: &[u8]| {
            b.push(code);
            b.push(data.len() as u8);
            b.extend_from_slice(data);
        };
        opt(OPT_MESSAGE_TYPE, &[self.kind as u8]);
        let mut client_id = vec![1];
        client_id.extend_from_slice(&self.mac);
        opt(OPT_CLIENT_ID, &client_id);
        opt(OPT_MAX_SIZE, &1500u16.to_be_bytes());
        if !self.hostname.is_empty() {
            opt(OPT_HOSTNAME, &self.hostname.as_bytes()[..self.hostname.len().min(255)]);
        }
        if let Some(ip) = self.requested {
            opt(OPT_REQUESTED_IP, &ip.octets());
        }
        if let Some(ip) = self.server {
            opt(OPT_SERVER_ID, &ip.octets());
        }
        opt(
            OPT_PARAMS,
            &[OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_DOMAIN, OPT_LEASE_TIME, OPT_RENEWAL, OPT_REBINDING],
        );
        b.push(OPT_END);
        if b.len() < MIN_LEN {
            b.resize(MIN_LEN, OPT_PAD);
        }
        b
    }
}

/// A server's reply, with the fields the client uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub kind: MessageType,
    pub xid: u32,
    pub mac: [u8; 6],
    pub yiaddr: Ipv4Addr,
    pub server: Option<Ipv4Addr>,
    pub netmask: Option<Ipv4Addr>,
    pub router: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    pub domain: Option<String>,
    pub lease: Option<Duration>,
    pub renewal: Option<Duration>,
    pub rebinding: Option<Duration>,
}

impl Reply {
    /// Decodes a reply; anything that is not a well-formed BOOTREPLY is `None`.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < 240 || b[0] != 2 || b[1] != 1 || b[2] != 6 || b[236..240] != MAGIC_COOKIE {
            return None;
        }
        let ip = |s: &[u8]| Ipv4Addr::new(s[0], s[1], s[2], s[3]);
        let secs = |s: &[u8]| Duration::from_secs(u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as u64);

        let mut r = Reply {
            kind: MessageType::Offer,
            xid: u32::from_be_bytes(b[4..8].try_into().ok()?),
            mac: b[28..34].try_into().ok()?,
            yiaddr: ip(&b[16..20]),
            server: None,
            netmask: None,
            router: None,
            dns: Vec::new(),
            domain: None,
            lease: None,
            renewal: None,
            rebinding: None,
        };
        let mut kind = None;
        let mut i = 240;
        while i < b.len() {
            let code = b[i];
            if code == OPT_PAD {
                i += 1;
                continue;
            }
            if code == OPT_END {
                break;
            }
            let len = *b.get(i + 1)? as usize;
            let data = b.get(i + 2..i + 2 + len)?;
            match (code, len) {
                (OPT_MESSAGE_TYPE, 1) => kind = MessageType::from_u8(data[0]),
                (OPT_SERVER_ID, 4) => r.server = Some(ip(data)),
                (OPT_SUBNET_MASK, 4) => r.netmask = Some(ip(data)),
                (OPT_ROUTER, 4..) => r.router = Some(ip(data)),
                (OPT_DNS, _) => r.dns = data.as_chunks::<4>().0.iter().map(|c| ip(c)).collect(),
                (OPT_DOMAIN, _) => r.domain = std::str::from_utf8(data).ok().map(str::to_owned),
                (OPT_LEASE_TIME, 4) => r.lease = Some(secs(data)),
                (OPT_RENEWAL, 4) => r.renewal = Some(secs(data)),
                (OPT_REBINDING, 4) => r.rebinding = Some(secs(data)),
                _ => {}
            }
            i += 2 + len;
        }
        r.kind = kind?;
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    fn reply_bytes(kind: MessageType, xid: u32) -> Vec<u8> {
        let mut b = vec![0u8; 236];
        b[0] = 2;
        b[1] = 1;
        b[2] = 6;
        b[4..8].copy_from_slice(&xid.to_be_bytes());
        b[16..20].copy_from_slice(&[10, 0, 2, 15]);
        b[28..34].copy_from_slice(&MAC);
        b.extend_from_slice(&MAGIC_COOKIE);
        b.extend_from_slice(&[OPT_MESSAGE_TYPE, 1, kind as u8]);
        b.extend_from_slice(&[OPT_SERVER_ID, 4, 10, 0, 2, 2]);
        b.extend_from_slice(&[OPT_SUBNET_MASK, 4, 255, 255, 255, 0]);
        b.extend_from_slice(&[OPT_ROUTER, 4, 10, 0, 2, 2]);
        b.extend_from_slice(&[OPT_DNS, 8, 10, 0, 2, 3, 1, 1, 1, 1]);
        b.extend_from_slice(&[OPT_PAD, OPT_PAD]);
        b.extend_from_slice(&[OPT_LEASE_TIME, 4, 0, 1, 81, 128]);
        b.push(OPT_END);
        b
    }

    #[test]
    fn request_is_a_padded_broadcast_bootrequest() {
        let b = Request {
            kind: MessageType::Discover,
            xid: 0xdead_beef,
            mac: MAC,
            hostname: "neslina-3456",
            ciaddr: Ipv4Addr::UNSPECIFIED,
            requested: None,
            server: None,
        }
        .encode();
        assert!(b.len() >= MIN_LEN);
        assert_eq!(b[0], 1);
        assert_eq!(&b[4..8], &0xdead_beefu32.to_be_bytes());
        assert_eq!(&b[10..12], &[0x80, 0]);
        assert_eq!(&b[28..34], &MAC);
        assert_eq!(&b[236..240], &MAGIC_COOKIE);
        assert_eq!(&b[240..243], &[OPT_MESSAGE_TYPE, 1, 1]);
        let host = b.windows(14).any(|w| w[0] == OPT_HOSTNAME && w[1] == 12 && &w[2..] == b"neslina-3456");
        assert!(host);
    }

    #[test]
    fn request_carries_requested_ip_and_server() {
        let b = Request {
            kind: MessageType::Request,
            xid: 1,
            mac: MAC,
            hostname: "",
            ciaddr: Ipv4Addr::UNSPECIFIED,
            requested: Some(Ipv4Addr::new(10, 0, 2, 15)),
            server: Some(Ipv4Addr::new(10, 0, 2, 2)),
        }
        .encode();
        assert!(b.windows(6).any(|w| w == [OPT_REQUESTED_IP, 4, 10, 0, 2, 15]));
        assert!(b.windows(6).any(|w| w == [OPT_SERVER_ID, 4, 10, 0, 2, 2]));
        assert!(!option_codes(&b).contains(&OPT_HOSTNAME));
    }

    fn option_codes(b: &[u8]) -> Vec<u8> {
        let mut codes = Vec::new();
        let mut i = 240;
        while i < b.len() && b[i] != OPT_END {
            if b[i] == OPT_PAD {
                i += 1;
                continue;
            }
            codes.push(b[i]);
            i += 2 + b[i + 1] as usize;
        }
        codes
    }

    #[test]
    fn decodes_an_ack() {
        let r = Reply::decode(&reply_bytes(MessageType::Ack, 7)).unwrap();
        assert_eq!(r.kind, MessageType::Ack);
        assert_eq!(r.xid, 7);
        assert_eq!(r.mac, MAC);
        assert_eq!(r.yiaddr, Ipv4Addr::new(10, 0, 2, 15));
        assert_eq!(r.server, Some(Ipv4Addr::new(10, 0, 2, 2)));
        assert_eq!(r.netmask, Some(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(r.router, Some(Ipv4Addr::new(10, 0, 2, 2)));
        assert_eq!(r.dns, vec![Ipv4Addr::new(10, 0, 2, 3), Ipv4Addr::new(1, 1, 1, 1)]);
        assert_eq!(r.lease, Some(Duration::from_secs(86400)));
        assert_eq!(r.renewal, None);
    }

    #[test]
    fn rejects_requests_truncation_and_missing_type() {
        let mut ours = reply_bytes(MessageType::Ack, 7);
        ours[0] = 1;
        assert!(Reply::decode(&ours).is_none());

        let full = reply_bytes(MessageType::Ack, 7);
        assert!(Reply::decode(&full[..238]).is_none());
        // An option whose length runs past the end.
        let mut cut = full[..244].to_vec();
        cut.extend_from_slice(&[OPT_DNS, 40, 1, 2]);
        assert!(Reply::decode(&cut).is_none());

        let mut no_type = full.clone();
        no_type[240..243].copy_from_slice(&[OPT_PAD, OPT_PAD, OPT_PAD]);
        assert!(Reply::decode(&no_type).is_none());
    }
}
