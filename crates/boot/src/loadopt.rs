//! `EFI_LOAD_OPTION`, the value of a `Boot####` variable (UEFI spec 3.1.3).
//!
//! Only the shape neslina writes is built: a short-form hard-drive device path
//! (the partition, found by its GPT GUID) followed by the file path on it.
//! Firmware expands the short form itself, as it does for the entries
//! `efibootmgr` writes.

use crate::gpt::GptPartition;

const LOAD_OPTION_ACTIVE: u32 = 0x1;

const MEDIA: u8 = 0x04;
const MEDIA_HARD_DRIVE: u8 = 0x01;
const MEDIA_FILE_PATH: u8 = 0x04;
const END: u8 = 0x7f;
const END_ENTIRE: u8 = 0xff;
const HD_NODE_LEN: u16 = 42;
const HD_FORMAT_GPT: u8 = 0x02;
const HD_SIGNATURE_GUID: u8 = 0x02;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOption {
    pub description: String,
    pub partition: Option<GptPartition>,
    /// A path on that partition, with backslashes, as the firmware sees it.
    pub file: Option<String>,
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

fn node(kind: u8, sub: u8, data: &[u8]) -> Vec<u8> {
    let mut n = vec![kind, sub];
    n.extend_from_slice(&((4 + data.len()) as u16).to_le_bytes());
    n.extend_from_slice(data);
    n
}

impl LoadOption {
    pub fn encode(&self) -> Vec<u8> {
        let mut path = Vec::new();
        if let Some(p) = &self.partition {
            let mut d = Vec::with_capacity(HD_NODE_LEN as usize - 4);
            d.extend_from_slice(&p.number.to_le_bytes());
            d.extend_from_slice(&p.start_lba.to_le_bytes());
            d.extend_from_slice(&p.size_lba.to_le_bytes());
            d.extend_from_slice(&p.guid);
            d.push(HD_FORMAT_GPT);
            d.push(HD_SIGNATURE_GUID);
            path.extend(node(MEDIA, MEDIA_HARD_DRIVE, &d));
        }
        if let Some(f) = &self.file {
            path.extend(node(MEDIA, MEDIA_FILE_PATH, &utf16z(f)));
        }
        path.extend(node(END, END_ENTIRE, &[]));

        let mut out = LOAD_OPTION_ACTIVE.to_le_bytes().to_vec();
        out.extend_from_slice(&(path.len() as u16).to_le_bytes());
        out.extend(utf16z(&self.description));
        out.extend(path);
        out
    }

    /// Decodes any load option far enough to compare it with ours; device
    /// path nodes neslina does not write are skipped.
    pub fn decode(b: &[u8]) -> Option<Self> {
        let path_len = u16::from_le_bytes(b.get(4..6)?.try_into().ok()?) as usize;
        let (description, rest) = read_utf16z(b.get(6..)?)?;
        let mut path = rest.get(..path_len)?;
        let mut opt = LoadOption { description, partition: None, file: None };
        while path.len() >= 4 {
            let (kind, sub) = (path[0], path[1]);
            let len = u16::from_le_bytes([path[2], path[3]]) as usize;
            if len < 4 || len > path.len() {
                return None;
            }
            let data = &path[4..len];
            match (kind, sub) {
                (END, END_ENTIRE) => break,
                (MEDIA, MEDIA_HARD_DRIVE)
                    if len == HD_NODE_LEN as usize && data[36] == HD_FORMAT_GPT && data[37] == HD_SIGNATURE_GUID =>
                {
                    opt.partition = Some(GptPartition {
                        number: u32::from_le_bytes(data[0..4].try_into().ok()?),
                        start_lba: u64::from_le_bytes(data[4..12].try_into().ok()?),
                        size_lba: u64::from_le_bytes(data[12..20].try_into().ok()?),
                        guid: data[20..36].try_into().ok()?,
                    });
                }
                (MEDIA, MEDIA_FILE_PATH) => opt.file = read_utf16z(data).map(|(s, _)| s),
                _ => {}
            }
            path = &path[len..];
        }
        Some(opt)
    }
}

/// A NUL-terminated UTF-16LE string and what follows it.
fn read_utf16z(b: &[u8]) -> Option<(String, &[u8])> {
    let mut units = Vec::new();
    let mut i = 0;
    loop {
        let u = u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?);
        i += 2;
        if u == 0 {
            break;
        }
        units.push(u);
    }
    Some((String::from_utf16(&units).ok()?, &b[i..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours() -> LoadOption {
        LoadOption {
            description: "neslina a".into(),
            partition: Some(GptPartition { number: 1, start_lba: 2048, size_lba: 2_048_000, guid: [0xab; 16] }),
            file: Some(r"\neslina\a.efi".into()),
        }
    }

    #[test]
    fn round_trips() {
        assert_eq!(LoadOption::decode(&ours().encode()), Some(ours()));
    }

    #[test]
    fn layout_matches_the_spec() {
        let b = ours().encode();
        assert_eq!(&b[0..4], &1u32.to_le_bytes());
        let desc_len = ("neslina a".len() + 1) * 2;
        let path = &b[6 + desc_len..];
        assert_eq!(u16::from_le_bytes([b[4], b[5]]) as usize, path.len());
        // Hard-drive node, 42 bytes, GPT with a GUID signature.
        assert_eq!(&path[0..4], &[4, 1, 42, 0]);
        assert_eq!(&path[40..42], &[2, 2]);
        // File path node follows, then the end node.
        assert_eq!(&path[42..44], &[4, 4]);
        assert_eq!(&path[path.len() - 4..], &[0x7f, 0xff, 4, 0]);
    }

    #[test]
    fn skips_nodes_it_does_not_know() {
        // A firmware entry: description, then a PCI node (1/1) and the end node.
        let mut b = 1u32.to_le_bytes().to_vec();
        let path = [1, 1, 6, 0, 0, 2, 0x7f, 0xff, 4, 0];
        b.extend_from_slice(&(path.len() as u16).to_le_bytes());
        b.extend(utf16z("UEFI QEMU USB"));
        b.extend_from_slice(&path);
        let o = LoadOption::decode(&b).unwrap();
        assert_eq!(o.description, "UEFI QEMU USB");
        assert_eq!((o.partition, o.file), (None, None));
    }

    #[test]
    fn rejects_truncated_input() {
        let b = ours().encode();
        assert!(LoadOption::decode(&b[..b.len() - 10]).is_none());
        assert!(LoadOption::decode(&b[..3]).is_none());
    }
}
