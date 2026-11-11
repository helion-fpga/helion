//! Minimal store-only (method 0) ZIP writer and reader.
//!
//! The workspace has no zip dependency; project archives and hardware
//! platform hand-offs only need an uncompressed, universally readable
//! container (`unzip`, Python `zipfile`, Finder and Explorer all read stored
//! entries). Entries carry a CRC-32 and a DOS timestamp; names are UTF-8
//! (general-purpose bit 11). No ZIP64: archives are capped at 4 GiB.

use std::io::Write;
use std::path::Path;

fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, slot) in t.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    t
}

/// IEEE 802.3 CRC-32 (the ZIP checksum).
pub fn crc32(data: &[u8]) -> u32 {
    let t = crc32_table();
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// MS-DOS (time, date) for a Unix timestamp (UTC).
pub fn dos_datetime(unix_secs: u64) -> (u16, u16) {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let year = (y - 1980).clamp(0, 127) as u16;
    let date = (year << 9) | ((m as u16) << 5) | d as u16;
    let time = (((rem / 3600) as u16) << 11) | ((((rem / 60) % 60) as u16) << 5) | ((rem % 60) / 2) as u16;
    (time, date)
}

struct Entry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

/// Streaming store-only ZIP writer.
pub struct ZipWriter<W: Write> {
    out: W,
    pos: u64,
    entries: Vec<Entry>,
    time: u16,
    date: u16,
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(315_532_800);
        let (time, date) = dos_datetime(now);
        Self {
            out,
            pos: 0,
            entries: Vec::new(),
            time,
            date,
        }
    }

    fn put(&mut self, b: &[u8]) -> Result<(), String> {
        self.out.write_all(b).map_err(|e| format!("zip write: {e}"))?;
        self.pos += b.len() as u64;
        Ok(())
    }

    /// Add one file. `name` uses `/` separators and must be relative.
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        if name.is_empty() || name.starts_with('/') || name.split('/').any(|c| c == "..") {
            return Err(format!("zip: unsafe entry name {name:?}"));
        }
        if self.entries.iter().any(|e| e.name == name) {
            return Err(format!("zip: duplicate entry {name}"));
        }
        if data.len() > u32::MAX as usize || self.pos > u32::MAX as u64 {
            return Err("zip: archive exceeds 4 GiB (no ZIP64)".into());
        }
        let crc = crc32(data);
        let offset = self.pos as u32;
        let mut h = Vec::with_capacity(30 + name.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&0x0800u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&self.time.to_le_bytes());
        h.extend_from_slice(&self.date.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        self.put(&h)?;
        self.put(data)?;
        self.entries.push(Entry {
            name: name.to_string(),
            crc,
            size: data.len() as u32,
            offset,
        });
        Ok(())
    }

    /// Write the central directory; returns the inner writer.
    pub fn finish(mut self) -> Result<W, String> {
        let cd_start = self.pos as u32;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let mut c = Vec::with_capacity(46 + e.name.len());
            c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            c.extend_from_slice(&0x031Eu16.to_le_bytes()); // made by: Unix, 3.0
            c.extend_from_slice(&20u16.to_le_bytes());
            c.extend_from_slice(&0x0800u16.to_le_bytes());
            c.extend_from_slice(&0u16.to_le_bytes());
            c.extend_from_slice(&self.time.to_le_bytes());
            c.extend_from_slice(&self.date.to_le_bytes());
            c.extend_from_slice(&e.crc.to_le_bytes());
            c.extend_from_slice(&e.size.to_le_bytes());
            c.extend_from_slice(&e.size.to_le_bytes());
            c.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            c.extend_from_slice(&0u16.to_le_bytes()); // extra
            c.extend_from_slice(&0u16.to_le_bytes()); // comment
            c.extend_from_slice(&0u16.to_le_bytes()); // disk
            c.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            c.extend_from_slice(&(0o100644u32 << 16).to_le_bytes());
            c.extend_from_slice(&e.offset.to_le_bytes());
            c.extend_from_slice(e.name.as_bytes());
            self.put(&c)?;
        }
        let cd_size = self.pos as u32 - cd_start;
        let n = entries.len() as u16;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&n.to_le_bytes());
        end.extend_from_slice(&n.to_le_bytes());
        end.extend_from_slice(&cd_size.to_le_bytes());
        end.extend_from_slice(&cd_start.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.put(&end)?;
        self.out.flush().map_err(|e| format!("zip flush: {e}"))?;
        Ok(self.out)
    }
}

/// Read every entry of a stored (method 0) ZIP, verifying CRCs, via the
/// central directory. Used by tests and by consumers of the hand-off formats.
pub fn read_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let u16_at = |p: usize| -> Result<u16, String> {
        bytes
            .get(p..p + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| "zip: truncated".to_string())
    };
    let u32_at = |p: usize| -> Result<u32, String> {
        bytes
            .get(p..p + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "zip: truncated".to_string())
    };
    if bytes.len() < 22 {
        return Err("zip: too short".into());
    }
    let eocd = (0..=bytes.len() - 22)
        .rev()
        .find(|&p| u32_at(p).ok() == Some(0x0605_4b50))
        .ok_or("zip: no end-of-central-directory record")?;
    let n = u16_at(eocd + 10)? as usize;
    let mut p = u32_at(eocd + 16)? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if u32_at(p)? != 0x0201_4b50 {
            return Err("zip: bad central directory".into());
        }
        let method = u16_at(p + 10)?;
        let crc = u32_at(p + 16)?;
        let size = u32_at(p + 20)? as usize;
        let name_len = u16_at(p + 28)? as usize;
        let extra = u16_at(p + 30)? as usize;
        let comment = u16_at(p + 32)? as usize;
        let local = u32_at(p + 42)? as usize;
        let name = String::from_utf8(bytes.get(p + 46..p + 46 + name_len).ok_or("zip: truncated")?.to_vec())
            .map_err(|_| "zip: non-UTF-8 name")?;
        if method != 0 {
            return Err(format!("zip: {name} is compressed (method {method}); only stored entries are read"));
        }
        let lname = u16_at(local + 26)? as usize;
        let lextra = u16_at(local + 28)? as usize;
        let start = local + 30 + lname + lextra;
        let data = bytes.get(start..start + size).ok_or("zip: truncated data")?.to_vec();
        if crc32(&data) != crc {
            return Err(format!("zip: CRC mismatch in {name}"));
        }
        out.push((name, data));
        p += 46 + name_len + extra + comment;
    }
    Ok(out)
}

/// Extract a stored ZIP into `dir` (entries must be relative and safe).
pub fn extract_zip(bytes: &[u8], dir: &Path) -> Result<usize, String> {
    let entries = read_zip(bytes)?;
    for (name, data) in &entries {
        if name.starts_with('/') || name.split('/').any(|c| c == "..") {
            return Err(format!("zip: unsafe entry {name}"));
        }
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_dos_time_known_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        // 2024-02-29 13:45:30 UTC
        let (t, d) = dos_datetime(1_709_214_330);
        assert_eq!(d, ((2024 - 1980) << 9) | (2 << 5) | 29);
        assert_eq!(t, (13 << 11) | (45 << 5) | 15);
    }

    #[test]
    fn round_trip_and_rejects_unsafe_names() {
        let mut z = ZipWriter::new(Vec::new());
        z.add("a/b.txt", b"hello").unwrap();
        z.add("empty", b"").unwrap();
        assert!(z.add("../evil", b"x").is_err());
        assert!(z.add("/abs", b"x").is_err());
        assert!(z.add("a/b.txt", b"dup").is_err());
        let bytes = z.finish().unwrap();
        let e = read_zip(&bytes).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0], ("a/b.txt".to_string(), b"hello".to_vec()));
        let mut bad = bytes.clone();
        let at = bad.windows(5).position(|w| w == b"hello").unwrap();
        bad[at] = b'j';
        assert!(read_zip(&bad).unwrap_err().contains("CRC"));
    }
}
