//! Reading one partition's entry from a GPT, for the firmware's device path.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use neslina_common::medium::{Partition, read_trimmed};

/// What a hard-drive device path node needs. LBAs are in the disk's logical
/// block size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GptPartition {
    pub number: u32,
    pub start_lba: u64,
    pub size_lba: u64,
    /// The unique partition GUID, in its on-disk byte order.
    pub guid: [u8; 16],
}

pub fn lookup(part: &Partition) -> io::Result<GptPartition> {
    let disk = part.disk()?;
    let number = part.number()?;
    let block: u64 = read_trimmed(format!("/sys/block/{disk}/queue/logical_block_size"))?
        .parse()
        .map_err(|_| invalid("bad logical block size"))?;
    let mut dev = File::open(format!("/dev/{disk}"))?;
    let mut header = vec![0u8; block as usize];
    dev.seek(SeekFrom::Start(block))?;
    dev.read_exact(&mut header)?;
    let mut entry = vec![0u8; 128];
    let (entries_lba, entry_size) = parse_header(&header)?;
    if number == 0 || entry_size < 128 {
        return Err(invalid("bad partition number or entry size"));
    }
    dev.seek(SeekFrom::Start(entries_lba * block + u64::from(number - 1) * u64::from(entry_size)))?;
    dev.read_exact(&mut entry)?;
    parse_entry(number, &entry)
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Returns the first LBA of the entry array and the size of one entry.
fn parse_header(h: &[u8]) -> io::Result<(u64, u32)> {
    if h.len() < 92 || &h[0..8] != b"EFI PART" {
        return Err(invalid("no GPT header"));
    }
    let lba = u64::from_le_bytes(h[72..80].try_into().unwrap());
    let size = u32::from_le_bytes(h[84..88].try_into().unwrap());
    Ok((lba, size))
}

fn parse_entry(number: u32, e: &[u8]) -> io::Result<GptPartition> {
    if e[0..16].iter().all(|&b| b == 0) {
        return Err(invalid("partition entry is unused"));
    }
    let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
    let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
    if last < first {
        return Err(invalid("partition ends before it starts"));
    }
    Ok(GptPartition { number, start_lba: first, size_lba: last - first + 1, guid: e[16..32].try_into().unwrap() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_entry() {
        let mut h = vec![0u8; 512];
        h[0..8].copy_from_slice(b"EFI PART");
        h[72..80].copy_from_slice(&2u64.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        assert_eq!(parse_header(&h).unwrap(), (2, 128));

        let mut e = vec![0u8; 128];
        e[0] = 0x28; // any non-zero type GUID
        e[16..32].copy_from_slice(&[7; 16]);
        e[32..40].copy_from_slice(&2048u64.to_le_bytes());
        e[40..48].copy_from_slice(&2_050_047u64.to_le_bytes());
        let p = parse_entry(1, &e).unwrap();
        assert_eq!(p, GptPartition { number: 1, start_lba: 2048, size_lba: 2_048_000, guid: [7; 16] });
    }

    #[test]
    fn rejects_non_gpt_and_unused_entries() {
        assert!(parse_header(&[0u8; 512]).is_err());
        assert!(parse_entry(1, &[0u8; 128]).is_err());
    }
}
