//! Writing a GUID partition table (UEFI spec 5.3).
//!
//! neslina only ever writes simple tables: a few partitions laid out in
//! order, each starting on a 1 MiB boundary. So this builds the whole table
//! in memory and returns where each part goes on the disk: protective MBR,
//! primary header and entries at the start, backup entries and header at the
//! end.

/// EFI system partition: the boot medium in internal mode.
pub const TYPE_ESP: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
/// Linux filesystem data: the state and games disks.
pub const TYPE_LINUX: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";

const ENTRIES: u32 = 128;
const ENTRY_SIZE: u32 = 128;
const HEADER_SIZE: u32 = 92;
const ALIGN_BYTES: u64 = 1 << 20;

pub struct Part {
    pub name: String,
    pub type_guid: [u8; 16],
    /// Size in bytes; `None` takes the rest of the disk. Only the last may be `None`.
    pub size: Option<u64>,
}

/// One write: `data` at byte `offset`.
pub struct Write {
    pub offset: u64,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub first_lba: u64,
    pub last_lba: u64,
}

/// A GUID string in the mixed-endian byte order GPT stores.
pub fn guid(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut b = [0u8; 16];
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    b[0..4].reverse();
    b[4..6].reverse();
    b[6..8].reverse();
    Some(b)
}

/// A random (version 4) GUID.
pub fn random_guid() -> [u8; 16] {
    let mut b = [0u8; 16];
    // SAFETY: getrandom fills at most b.len() bytes.
    unsafe { libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0) };
    b[7] = (b[7] & 0x0f) | 0x40; // version 4, in the little-endian third field
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    b
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Lays out `parts` on a disk of `sectors` logical blocks of `block` bytes.
/// Returns the writes that make the table and where each partition landed.
pub fn build(
    sectors: u64,
    block: u64,
    disk_guid: [u8; 16],
    parts: &[Part],
) -> Result<(Vec<Write>, Vec<Placed>), String> {
    let entries_bytes = (ENTRIES * ENTRY_SIZE) as u64;
    let entries_sectors = entries_bytes.div_ceil(block);
    let first_usable = 2 + entries_sectors;
    if sectors < first_usable * 2 + 2 {
        return Err("disk too small".into());
    }
    let last_usable = sectors - 2 - entries_sectors;
    let align = (ALIGN_BYTES / block).max(1);

    let mut placed = Vec::new();
    let mut next = first_usable.div_ceil(align) * align;
    for (i, p) in parts.iter().enumerate() {
        let size_sectors = match p.size {
            Some(bytes) => bytes.div_ceil(block),
            None if i == parts.len() - 1 => {
                // The rest, ending on an alignment boundary.
                let end = (last_usable + 1) / align * align;
                end.checked_sub(next).ok_or("no room left for the last partition")?
            }
            None => return Err("only the last partition may take the rest".into()),
        };
        if size_sectors == 0 || next + size_sectors - 1 > last_usable {
            return Err(format!("partition {} does not fit", p.name));
        }
        placed.push(Placed { first_lba: next, last_lba: next + size_sectors - 1 });
        next = (next + size_sectors).div_ceil(align) * align;
    }

    let mut entries = vec![0u8; entries_bytes as usize];
    for (i, (p, at)) in parts.iter().zip(&placed).enumerate() {
        let e = &mut entries[i * ENTRY_SIZE as usize..(i + 1) * ENTRY_SIZE as usize];
        e[0..16].copy_from_slice(&p.type_guid);
        e[16..32].copy_from_slice(&random_guid());
        e[32..40].copy_from_slice(&at.first_lba.to_le_bytes());
        e[40..48].copy_from_slice(&at.last_lba.to_le_bytes());
        let name: Vec<u16> = p.name.encode_utf16().take(36).collect();
        for (j, unit) in name.iter().enumerate() {
            e[56 + j * 2..58 + j * 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    let entries_crc = crc32(&entries);

    let header = |current: u64, backup: u64, entries_lba: u64| {
        let mut h = vec![0u8; block as usize];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&HEADER_SIZE.to_le_bytes());
        h[24..32].copy_from_slice(&current.to_le_bytes());
        h[32..40].copy_from_slice(&backup.to_le_bytes());
        h[40..48].copy_from_slice(&first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[56..72].copy_from_slice(&disk_guid);
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&ENTRIES.to_le_bytes());
        h[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let crc = crc32(&h[..HEADER_SIZE as usize]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    };

    // Protective MBR: one partition of type 0xEE covering the disk.
    let mut mbr = vec![0u8; block as usize];
    let p = &mut mbr[446..462];
    p[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    p[4] = 0xEE;
    p[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    p[8..12].copy_from_slice(&1u32.to_le_bytes());
    p[12..16].copy_from_slice(&((sectors - 1).min(u32::MAX as u64) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    let backup_entries_lba = sectors - 1 - entries_sectors;
    let writes = vec![
        Write { offset: 0, data: mbr },
        Write { offset: block, data: header(1, sectors - 1, 2) },
        Write { offset: 2 * block, data: entries.clone() },
        Write { offset: backup_entries_lba * block, data: entries },
        Write { offset: (sectors - 1) * block, data: header(sectors - 1, 1, backup_entries_lba) },
    ];
    Ok((writes, placed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(sectors: u64, block: u64, parts: &[Part]) -> (Vec<u8>, Vec<Placed>) {
        let (writes, placed) = build(sectors, block, random_guid(), parts).unwrap();
        let mut disk = vec![0u8; (sectors * block) as usize];
        for w in writes {
            disk[w.offset as usize..w.offset as usize + w.data.len()].copy_from_slice(&w.data);
        }
        (disk, placed)
    }

    fn u64_at(b: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn guid_uses_mixed_endian() {
        let esp = guid(TYPE_ESP).unwrap();
        assert_eq!(
            esp,
            [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]
        );
        assert!(guid("nope").is_none());
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn one_partition_fills_the_disk_aligned() {
        let sectors = 64 * 2048; // 64 MiB of 512-byte blocks
        let parts = [Part { name: "NESLINA-STATE".into(), type_guid: guid(TYPE_LINUX).unwrap(), size: None }];
        let (disk, placed) = image(sectors, 512, &parts);
        assert_eq!(placed, vec![Placed { first_lba: 2048, last_lba: 63 * 2048 - 1 }]);

        // Primary and backup headers point at each other and carry valid CRCs.
        let primary = &disk[512..1024];
        let backup = &disk[((sectors - 1) * 512) as usize..(sectors * 512) as usize];
        for (h, me, other) in [(primary, 1, sectors - 1), (backup, sectors - 1, 1)] {
            assert_eq!(&h[0..8], b"EFI PART");
            assert_eq!(u64_at(h, 24), me);
            assert_eq!(u64_at(h, 32), other);
            let mut copy = h[..92].to_vec();
            let stored = u32::from_le_bytes(copy[16..20].try_into().unwrap());
            copy[16..20].fill(0);
            assert_eq!(crc32(&copy), stored);
        }
        assert_eq!(u64_at(primary, 72), 2);
        assert_eq!(u64_at(backup, 72), sectors - 33);
        // Both entry arrays are the same and match the CRC in the headers.
        let entries = &disk[1024..1024 + 16384];
        let backup_entries = &disk[((sectors - 33) * 512) as usize..((sectors - 33) * 512) as usize + 16384];
        assert_eq!(entries, backup_entries);
        assert_eq!(crc32(entries), u32::from_le_bytes(primary[88..92].try_into().unwrap()));
        // The entry itself, with its UTF-16 name.
        assert_eq!(&entries[0..16], &guid(TYPE_LINUX).unwrap());
        assert_eq!(&entries[56..62], &[b'N', 0, b'E', 0, b'S', 0]);
        // Protective MBR.
        assert_eq!(disk[446 + 4], 0xEE);
        assert_eq!(&disk[510..512], &[0x55, 0xAA]);
    }

    #[test]
    fn partitions_follow_each_other_on_boundaries() {
        let parts = [
            Part { name: "NESLINA".into(), type_guid: guid(TYPE_ESP).unwrap(), size: Some(1 << 30) },
            Part { name: "NESLINA-STATE".into(), type_guid: guid(TYPE_LINUX).unwrap(), size: None },
        ];
        // 8 GiB of 4096-byte blocks: alignment is 256 blocks.
        let (_, placed) = image(8 * 262_144, 4096, &parts);
        assert_eq!(placed[0], Placed { first_lba: 256, last_lba: 256 + 262_144 - 1 });
        assert_eq!(placed[1].first_lba, 256 + 262_144);
        assert_eq!((placed[1].last_lba + 1) % 256, 0);
    }

    #[test]
    fn refuses_what_does_not_fit() {
        let big = [Part { name: "x".into(), type_guid: [1; 16], size: Some(1 << 40) }];
        assert!(build(2048 * 64, 512, [0; 16], &big).is_err());
        let rest_first = [
            Part { name: "a".into(), type_guid: [1; 16], size: None },
            Part { name: "b".into(), type_guid: [1; 16], size: Some(1 << 20) },
        ];
        assert!(build(2048 * 64, 512, [0; 16], &rest_first).is_err());
    }
}
