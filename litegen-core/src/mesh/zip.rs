//! ZIP archives: the outer container of USDZ and 3MF.
//!
//! # Reading untrusted archives
//!
//! A ZIP is a directory of offsets and sizes chosen by whoever wrote it, and
//! every classic archive attack lives in trusting them: an entry whose declared
//! size is small and whose deflate stream inflates to gigabytes (a zip bomb), a
//! name like `../../etc/passwd`, two entries with one name so that two readers
//! disagree about which is "the" file, a local header that disagrees with the
//! central directory. So:
//!
//! - the central directory is the authority (it is what every mainstream reader
//!   uses), located from the end-of-central-directory record;
//! - entries are inflated lazily, one at a time, only when asked for, with the
//!   output capped at the size the directory declared and the CRC checked;
//! - names are normalised by [`super::package::normalize_relative`], and a name
//!   that escapes the archive or appears twice is an error;
//! - encryption and multi-disk archives are refused: neither occurs in the
//!   files this module exists for, and each is a parser of its own;
//! - ZIP64 records are read, because 3MF consumers "MUST support ... the
//!   PKWARE ZIP64 extension" (3MF Core §1.1) and real writers (assimp) add
//!   them to every archive; with input capped at `MAX_INPUT_BYTES`, every
//!   64-bit value must still fit the 32-bit world it describes.
//!
//! # Writing
//!
//! Entries can be stored or deflated. Stored entries can be aligned — USDZ
//! requires every file's data to start on a 64-byte boundary so that a reader
//! can map it in place — which is done the way Pixar's own writer does it: an
//! extra field in the local header, sized so the data lands on the boundary.
//! Timestamps are fixed (1980-01-01) so the same model always zips to the same
//! bytes.
//!
//! @see <https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT>

use super::package;
use super::MeshError;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_ZIP64_LOCATOR: u32 = 0x0706_4b50;
const SIG_ZIP64_EOCD: u32 = 0x0606_4b50;
/// Extra-field id of ZIP64 extended information (APPNOTE 4.5.3).
const EXTRA_ZIP64: u16 = 0x0001;
/// A 16-bit or 32-bit field that is all ones defers to its ZIP64 counterpart.
const SATURATED16: u64 = 0xFFFF;
const SATURATED32: u64 = 0xFFFF_FFFF;

const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

/// Most entries a single archive may list. A USDZ or 3MF is a model, a
/// handful of textures and some metadata; tens of thousands of entries is
/// a directory scan waiting to happen.
const MAX_ENTRIES: usize = 10_000;
/// Total bytes all entries may inflate to, together.
const MAX_TOTAL_UNCOMPRESSED: u64 = 4 * super::MAX_INPUT_BYTES as u64;

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("zip: {}", detail.into()))
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(format!("zip: {}", detail.into()))
}

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off.checked_add(8)?)?;
    Some(u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// The data of the ZIP64 extended-information block in an extra field.
fn zip64_extra(extra: &[u8]) -> Option<&[u8]> {
    let mut at = 0usize;
    while at + 4 <= extra.len() {
        let id = u16_at(extra, at)?;
        let len = u16_at(extra, at + 2)? as usize;
        let data = extra.get(at + 4..at + 4 + len)?;
        if id == EXTRA_ZIP64 {
            return Some(data);
        }
        at += 4 + len;
    }
    None
}

/// CRC-32 (IEEE 802.3), as ZIP and PNG use it.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Whether these bytes start like a ZIP archive at all.
pub fn looks_like_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 22 && u32_at(bytes, 0) == Some(SIG_LOCAL)
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Normalised, archive-relative path.
    pub name: String,
    method: u16,
    crc: u32,
    compressed: usize,
    uncompressed: usize,
    /// Offset of the entry's data (past its local header).
    data_offset: usize,
}

impl Entry {
    pub fn uncompressed_size(&self) -> usize {
        self.uncompressed
    }

    pub fn is_stored(&self) -> bool {
        self.method == METHOD_STORED
    }

    /// Absolute offset of the entry's data within the archive.
    pub fn data_offset(&self) -> usize {
        self.data_offset
    }
}

/// A parsed archive. Entries are listed in central-directory order, which for
/// every writer that matters is the order they were written in — USDZ relies on
/// that to name its root layer.
pub struct Archive<'a> {
    bytes: &'a [u8],
    pub entries: Vec<Entry>,
}

impl<'a> Archive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Archive<'a>, MeshError> {
        if !looks_like_zip(bytes) {
            return Err(malformed("no local file header at the start of the archive"));
        }
        // The EOCD record is the last thing in the file, followed only by an
        // optional comment of at most 65535 bytes. Scanning backwards bounds the
        // search to that window.
        let min = bytes.len().saturating_sub(22 + 65_535);
        let mut eocd = None;
        let mut i = bytes.len().saturating_sub(22);
        loop {
            if u32_at(bytes, i) == Some(SIG_EOCD) {
                let comment_len = u16_at(bytes, i + 20).unwrap_or(0) as usize;
                // A real EOCD's comment runs exactly to the end of the file; a
                // stray signature inside the comment does not.
                if i + 22 + comment_len == bytes.len() {
                    eocd = Some(i);
                    break;
                }
            }
            if i == min || i == 0 {
                break;
            }
            i -= 1;
        }
        let eocd = eocd.ok_or_else(|| malformed("no end-of-central-directory record"))?;
        let field16 = |off: usize| u16_at(bytes, eocd + off).map(u64::from).ok_or_else(|| malformed("truncated EOCD"));
        let field32 = |off: usize| u32_at(bytes, eocd + off).map(u64::from).ok_or_else(|| malformed("truncated EOCD"));
        let (mut disk, mut cd_disk, mut count) = (field16(4)?, field16(6)?, field16(10)?);
        let (mut cd_size, mut cd_offset) = (field32(12)?, field32(16)?);
        // A field too small for its value is set to all ones and the value
        // lives in the ZIP64 end record (APPNOTE 4.4.1.4). Where the classic
        // field holds a real value it IS the value — assimp writes a ZIP64
        // record into every archive, however small — so only saturated fields
        // are taken from the record.
        let saturated = [disk, cd_disk, count].contains(&SATURATED16)
            || cd_size == SATURATED32
            || cd_offset == SATURATED32;
        if saturated {
            let locator = eocd
                .checked_sub(20)
                .filter(|l| u32_at(bytes, *l) == Some(SIG_ZIP64_LOCATOR))
                .ok_or_else(|| malformed("end-of-central-directory fields are saturated and there is no zip64 record"))?;
            let rec = u64_at(bytes, locator + 8)
                .and_then(|r| usize::try_from(r).ok())
                .filter(|r| r.checked_add(56).is_some_and(|end| end <= locator))
                .filter(|r| u32_at(bytes, *r) == Some(SIG_ZIP64_EOCD))
                .ok_or_else(|| malformed("the zip64 end record is missing or lies outside the archive"))?;
            let wide32 = |off: usize| u32_at(bytes, rec + off).map(u64::from).unwrap_or(1);
            let wide64 = |off: usize| u64_at(bytes, rec + off).unwrap_or(u64::MAX);
            if disk == SATURATED16 {
                disk = wide32(16);
            }
            if cd_disk == SATURATED16 {
                cd_disk = wide32(20);
            }
            if count == SATURATED16 {
                count = wide64(32);
            }
            if cd_size == SATURATED32 {
                cd_size = wide64(40);
            }
            if cd_offset == SATURATED32 {
                cd_offset = wide64(48);
            }
        }
        if disk != 0 || cd_disk != 0 {
            return Err(unsupported("multi-disk archives are not supported"));
        }
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        let cd_size = usize::try_from(cd_size).map_err(|_| malformed("central directory size overflows"))?;
        let cd_offset = usize::try_from(cd_offset).map_err(|_| malformed("central directory offset overflows"))?;
        if count > MAX_ENTRIES {
            return Err(unsupported(format!("{count} entries exceeds the {MAX_ENTRIES}-entry limit")));
        }
        let cd_end = cd_offset
            .checked_add(cd_size)
            .filter(|e| *e <= eocd)
            .ok_or_else(|| malformed("central directory lies outside the archive"))?;

        let mut entries = Vec::with_capacity(count);
        let mut total: u64 = 0;
        let mut pos = cd_offset;
        for n in 0..count {
            if pos + 46 > cd_end || u32_at(bytes, pos) != Some(SIG_CENTRAL) {
                return Err(malformed(format!("central directory entry {n} is missing or truncated")));
            }
            let flags = u16_at(bytes, pos + 8).unwrap_or(0);
            let method = u16_at(bytes, pos + 10).unwrap_or(0);
            let crc = u32_at(bytes, pos + 16).unwrap_or(0);
            let mut compressed = u32_at(bytes, pos + 20).map_or(0, u64::from);
            let mut uncompressed = u32_at(bytes, pos + 24).map_or(0, u64::from);
            let name_len = u16_at(bytes, pos + 28).unwrap_or(0) as usize;
            let extra_len = u16_at(bytes, pos + 30).unwrap_or(0) as usize;
            let comment_len = u16_at(bytes, pos + 32).unwrap_or(0) as usize;
            let mut local = u32_at(bytes, pos + 42).map_or(0, u64::from);
            let raw_name = bytes
                .get(pos + 46..pos + 46 + name_len)
                .ok_or_else(|| malformed(format!("entry {n} name is truncated")))?;
            let extra = bytes.get(pos + 46 + name_len..pos + 46 + name_len + extra_len).unwrap_or(&[]);
            // Saturated sizes and offset continue in the ZIP64 block, as
            // 64-bit values, in exactly this order and only those saturated.
            if [uncompressed, compressed, local].contains(&SATURATED32) {
                let wide = zip64_extra(extra)
                    .ok_or_else(|| malformed(format!("entry {n} has saturated sizes and no zip64 field")))?;
                let mut at = 0usize;
                for field in [&mut uncompressed, &mut compressed, &mut local] {
                    if *field == SATURATED32 {
                        *field = u64_at(wide, at).ok_or_else(|| malformed(format!("entry {n} zip64 field is truncated")))?;
                        at += 8;
                    }
                }
            }
            pos = pos + 46 + name_len + extra_len + comment_len;
            if pos > cd_end {
                return Err(malformed(format!("central directory entry {n} overruns the directory")));
            }
            if flags & 0x0001 != 0 {
                return Err(unsupported("encrypted entries are not supported"));
            }
            let name_text = String::from_utf8_lossy(raw_name).into_owned();
            // Directory entries carry no data and name nothing we read.
            if name_text.ends_with('/') && uncompressed == 0 {
                continue;
            }
            // Within `MAX_INPUT_BYTES`, an honest size or offset is far below
            // 4 GiB whatever field it was written in.
            let (Ok(compressed), Ok(uncompressed), Ok(local)) =
                (usize::try_from(compressed), usize::try_from(uncompressed), usize::try_from(local))
            else {
                return Err(malformed(format!("entry {n} declares a size or offset past the address space")));
            };
            let name = package::normalize_relative(&name_text)
                .ok_or_else(|| malformed(format!("entry name {:?} escapes the archive", truncate(&name_text, 80))))?;
            if entries.iter().any(|e: &Entry| e.name == name) {
                // Two readers can disagree about which duplicate wins; that is
                // the ambiguity smuggling attacks are built on.
                return Err(malformed(format!("entry {name:?} appears twice")));
            }
            if method != METHOD_STORED && method != METHOD_DEFLATE {
                return Err(unsupported(format!("entry {name:?} uses compression method {method}")));
            }
            if method == METHOD_STORED && compressed != uncompressed {
                return Err(malformed(format!("stored entry {name:?} declares different sizes")));
            }
            total = total.saturating_add(uncompressed as u64);
            if total > MAX_TOTAL_UNCOMPRESSED {
                return Err(unsupported(format!(
                    "entries inflate to more than {MAX_TOTAL_UNCOMPRESSED} bytes"
                )));
            }
            // The local header has its own name and extra lengths, which may
            // legitimately differ from the central copy (alignment padding).
            if u32_at(bytes, local) != Some(SIG_LOCAL) {
                return Err(malformed(format!("entry {name:?} local header is missing")));
            }
            let lname = u16_at(bytes, local + 26).ok_or_else(|| malformed("truncated local header"))? as usize;
            let lextra = u16_at(bytes, local + 28).ok_or_else(|| malformed("truncated local header"))? as usize;
            let data_offset = local + 30 + lname + lextra;
            let data_end = data_offset
                .checked_add(compressed)
                .filter(|e| *e <= bytes.len())
                .ok_or_else(|| malformed(format!("entry {name:?} data runs past the archive")))?;
            let _ = data_end;
            entries.push(Entry { name, method, crc, compressed, uncompressed, data_offset });
        }
        Ok(Archive { bytes, entries })
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// A stored entry's bytes without copying (USDZ's in-place reads).
    pub fn stored_slice(&self, e: &Entry) -> Option<&'a [u8]> {
        (e.method == METHOD_STORED).then(|| &self.bytes[e.data_offset..e.data_offset + e.compressed])
    }

    /// Extract one entry, verifying its size and CRC.
    pub fn read(&self, e: &Entry) -> Result<Vec<u8>, MeshError> {
        let raw = &self.bytes[e.data_offset..e.data_offset + e.compressed];
        let data = match e.method {
            METHOD_STORED => raw.to_vec(),
            _ => {
                // The limit is the declared size: an entry that inflates past
                // what the directory promised is lying about itself, and
                // stopping there is what makes a zip bomb cost nothing.
                let out = miniz_oxide::inflate::decompress_to_vec_with_limit(raw, e.uncompressed)
                    .map_err(|err| malformed(format!("entry {:?} does not inflate: {err:?}", e.name)))?;
                if out.len() != e.uncompressed {
                    return Err(malformed(format!(
                        "entry {:?} inflates to {} bytes, not the declared {}",
                        e.name,
                        out.len(),
                        e.uncompressed
                    )));
                }
                out
            }
        };
        if crc32(&data) != e.crc {
            return Err(malformed(format!("entry {:?} fails its CRC check", e.name)));
        }
        Ok(data)
    }

    pub fn read_name(&self, name: &str) -> Result<Option<Vec<u8>>, MeshError> {
        match self.find(name) {
            Some(e) => self.read(e).map(Some),
            None => Ok(None),
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// How one entry is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// Uncompressed, data aligned to `align` bytes (1 = no alignment).
    Stored { align: usize },
    Deflated,
}

/// Builds an archive in memory.
#[derive(Default)]
pub struct Writer {
    out: Vec<u8>,
    central: Vec<u8>,
    count: usize,
}

/// 1980-01-01 00:00 in MS-DOS format, the earliest date ZIP can express: a
/// fixed timestamp keeps output byte-identical between runs.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = (1 << 5) | 1;

/// Deflate level for `Store::Deflated`. A 3MF model part is XML in the
/// hundreds of megabytes at the scene ceiling, and level 6 compressed a
/// 123 MB part in 2.07 s where level 3 took 0.49 s for a file 6.6% larger
/// (level 1: 0.24 s, 48% larger). Measured with miniz_oxide, release build.
const DEFLATE_LEVEL: u8 = 3;

/// Extra-field id for alignment padding. Not registered with PKWARE; readers
/// skip extra blocks they do not know, which is what padding needs.
const PAD_EXTRA_ID: u16 = 0x1986;

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    pub fn add(&mut self, name: &str, data: &[u8], how: Store) -> Result<(), MeshError> {
        if self.count >= 0xFFFF {
            return Err(unsupported("too many entries for a non-zip64 archive"));
        }
        if data.len() > u32::MAX as usize || self.out.len() > u32::MAX as usize {
            return Err(unsupported("archive exceeds 4 GiB"));
        }
        let crc = crc32(data);
        let (method, payload): (u16, std::borrow::Cow<[u8]>) = match how {
            Store::Stored { .. } => (METHOD_STORED, std::borrow::Cow::Borrowed(data)),
            Store::Deflated => {
                (METHOD_DEFLATE, std::borrow::Cow::Owned(miniz_oxide::deflate::compress_to_vec(data, DEFLATE_LEVEL)))
            }
        };
        let name_bytes = name.as_bytes();
        let header_offset = self.out.len();
        let mut extra: Vec<u8> = Vec::new();
        if let Store::Stored { align } = how {
            if align > 1 {
                let unpadded = header_offset + 30 + name_bytes.len();
                let mut pad = (align - unpadded % align) % align;
                if pad != 0 {
                    // An extra block needs 4 bytes of header; if the gap is
                    // smaller, go round to the next boundary.
                    while pad < 4 {
                        pad += align;
                    }
                    extra.extend_from_slice(&PAD_EXTRA_ID.to_le_bytes());
                    extra.extend_from_slice(&((pad - 4) as u16).to_le_bytes());
                    extra.resize(pad, 0);
                }
            }
        }
        let flags: u16 = 0x0800; // names are UTF-8
        let version: u16 = 20;
        let w16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        let w32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());

        let o = &mut self.out;
        w32(o, SIG_LOCAL);
        w16(o, version);
        w16(o, flags);
        w16(o, method);
        w16(o, DOS_TIME);
        w16(o, DOS_DATE);
        w32(o, crc);
        w32(o, payload.len() as u32);
        w32(o, data.len() as u32);
        w16(o, name_bytes.len() as u16);
        w16(o, extra.len() as u16);
        o.extend_from_slice(name_bytes);
        o.extend_from_slice(&extra);
        o.extend_from_slice(&payload);

        let c = &mut self.central;
        w32(c, SIG_CENTRAL);
        w16(c, version);
        w16(c, version);
        w16(c, flags);
        w16(c, method);
        w16(c, DOS_TIME);
        w16(c, DOS_DATE);
        w32(c, crc);
        w32(c, payload.len() as u32);
        w32(c, data.len() as u32);
        w16(c, name_bytes.len() as u16);
        w16(c, 0); // no extra in the central copy
        w16(c, 0); // comment
        w16(c, 0); // disk
        w16(c, 0); // internal attributes
        w32(c, 0); // external attributes
        w32(c, header_offset as u32);
        c.extend_from_slice(name_bytes);
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.out.len();
        let cd_size = self.central.len();
        self.out.extend_from_slice(&self.central);
        let o = &mut self.out;
        o.extend_from_slice(&SIG_EOCD.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&(self.count as u16).to_le_bytes());
        o.extend_from_slice(&(self.count as u16).to_le_bytes());
        o.extend_from_slice(&(cd_size as u32).to_le_bytes());
        o.extend_from_slice(&(cd_offset as u32).to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn round_trips_stored_and_deflated_entries() {
        let mut w = Writer::new();
        w.add("a.txt", b"hello", Store::Stored { align: 1 }).unwrap();
        let big: Vec<u8> = (0..10_000u32).map(|i| (i % 7) as u8).collect();
        w.add("dir/b.bin", &big, Store::Deflated).unwrap();
        let bytes = w.finish();
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.entries.len(), 2);
        assert_eq!(a.read_name("a.txt").unwrap().unwrap(), b"hello");
        assert_eq!(a.read_name("dir/b.bin").unwrap().unwrap(), big);
        assert_eq!(a.entries[0].name, "a.txt", "directory order is write order");
    }

    #[test]
    fn stored_entries_can_be_aligned_to_64_bytes() {
        let mut w = Writer::new();
        for (i, n) in ["root.usdc", "t/a.png", "longer_name_to_shift.png"].iter().enumerate() {
            w.add(n, &vec![i as u8; 37 + i * 11], Store::Stored { align: 64 }).unwrap();
        }
        let bytes = w.finish();
        let a = Archive::parse(&bytes).unwrap();
        for e in &a.entries {
            assert_eq!(e.data_offset() % 64, 0, "{} at {}", e.name, e.data_offset());
            assert!(a.stored_slice(e).is_some());
        }
    }

    #[test]
    fn a_corrupted_entry_fails_its_crc() {
        let mut w = Writer::new();
        w.add("a.txt", b"hello world", Store::Stored { align: 1 }).unwrap();
        let mut bytes = w.finish();
        let at = 30 + 5; // past the local header and name
        bytes[at] ^= 0xFF;
        let a = Archive::parse(&bytes).unwrap();
        assert!(matches!(a.read(&a.entries[0]), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn an_entry_that_inflates_past_its_declared_size_is_refused() {
        let mut w = Writer::new();
        w.add("bomb", &vec![0u8; 100_000], Store::Deflated).unwrap();
        let mut bytes = w.finish();
        // Rewrite the central directory's uncompressed size down to 10.
        let cd = bytes.windows(4).rposition(|s| s == SIG_CENTRAL.to_le_bytes()).unwrap();
        bytes[cd + 24..cd + 28].copy_from_slice(&10u32.to_le_bytes());
        let a = Archive::parse(&bytes).unwrap();
        assert!(a.read(&a.entries[0]).is_err());
    }

    #[test]
    fn names_that_escape_the_archive_are_refused() {
        for bad in ["../evil.png", "/abs.png", "C:/x.png"] {
            let mut w = Writer::new();
            w.add(bad, b"x", Store::Stored { align: 1 }).unwrap();
            let bytes = w.finish();
            assert!(Archive::parse(&bytes).is_err(), "{bad}");
        }
    }

    #[test]
    fn duplicate_names_are_refused() {
        let mut w = Writer::new();
        w.add("a", b"1", Store::Stored { align: 1 }).unwrap();
        w.add("./a", b"2", Store::Stored { align: 1 }).unwrap();
        assert!(Archive::parse(&w.finish()).is_err());
    }

    /// `bytes` (an archive with no comment) with a ZIP64 end record and its
    /// locator inserted before the classic end record; `saturate` sets the
    /// classic fields to all ones so the ZIP64 record is their only source.
    fn with_zip64_end(mut bytes: Vec<u8>, saturate: bool) -> Vec<u8> {
        let eocd = bytes.len() - 22;
        let count = u16_at(&bytes, eocd + 10).unwrap() as u64;
        let cd_size = u32_at(&bytes, eocd + 12).unwrap() as u64;
        let cd_offset = u32_at(&bytes, eocd + 16).unwrap() as u64;
        let mut tail = bytes.split_off(eocd);
        let rec = bytes.len() as u64;
        bytes.extend_from_slice(&SIG_ZIP64_EOCD.to_le_bytes());
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&cd_size.to_le_bytes());
        bytes.extend_from_slice(&cd_offset.to_le_bytes());
        bytes.extend_from_slice(&SIG_ZIP64_LOCATOR.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&rec.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        if saturate {
            tail[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
            tail[10..12].copy_from_slice(&0xFFFFu16.to_le_bytes());
            tail[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
            tail[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        bytes.extend_from_slice(&tail);
        bytes
    }

    fn two_entries() -> Vec<u8> {
        let mut w = Writer::new();
        w.add("a.txt", b"hello", Store::Stored { align: 1 }).unwrap();
        w.add("b.bin", &[7u8; 300], Store::Deflated).unwrap();
        w.finish()
    }

    #[test]
    fn a_redundant_zip64_end_record_is_read_past() {
        // assimp 6 writes a ZIP64 end record into every archive, however
        // small, beside classic fields that hold the real values.
        let bytes = with_zip64_end(two_entries(), false);
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.read_name("a.txt").unwrap().unwrap(), b"hello");
        assert_eq!(a.read_name("b.bin").unwrap().unwrap(), vec![7u8; 300]);
    }

    #[test]
    fn saturated_end_fields_are_taken_from_the_zip64_record() {
        let bytes = with_zip64_end(two_entries(), true);
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.entries.len(), 2);
        assert_eq!(a.read_name("b.bin").unwrap().unwrap(), vec![7u8; 300]);
        // Saturated fields with nowhere to find the values: refused.
        let mut lone = two_entries();
        let eocd = lone.len() - 22;
        lone[eocd + 10..eocd + 12].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(matches!(Archive::parse(&lone), Err(MeshError::Malformed(_))));
        // A locator pointing past itself: refused, not followed.
        let mut bad = with_zip64_end(two_entries(), true);
        let loc = bad.len() - 22 - 20;
        bad[loc + 8..loc + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(Archive::parse(&bad), Err(MeshError::Malformed(_))));
    }

    /// One stored entry whose central-directory sizes are saturated and given
    /// in a ZIP64 extra block, the way streaming writers that always emit
    /// ZIP64 record them.
    fn zip64_sized_entry(data: &[u8], zip64_len: u16) -> Vec<u8> {
        let name = b"x.bin";
        let mut out = Vec::new();
        let w16 = |o: &mut Vec<u8>, v: u16| o.extend_from_slice(&v.to_le_bytes());
        let w32 = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
        w32(&mut out, SIG_LOCAL);
        for v in [45u16, 0, 0, 0, 0] {
            w16(&mut out, v);
        }
        w32(&mut out, crc32(data));
        w32(&mut out, data.len() as u32);
        w32(&mut out, data.len() as u32);
        w16(&mut out, name.len() as u16);
        w16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        let cd = out.len();
        w32(&mut out, SIG_CENTRAL);
        for v in [45u16, 45, 0, 0, 0, 0] {
            w16(&mut out, v);
        }
        w32(&mut out, crc32(data));
        w32(&mut out, u32::MAX);
        w32(&mut out, u32::MAX);
        w16(&mut out, name.len() as u16);
        w16(&mut out, 4 + zip64_len);
        for v in [0u16, 0, 0] {
            w16(&mut out, v);
        }
        w32(&mut out, 0);
        w32(&mut out, 0);
        out.extend_from_slice(name);
        w16(&mut out, EXTRA_ZIP64);
        w16(&mut out, zip64_len);
        let mut wide = Vec::new();
        wide.extend_from_slice(&(data.len() as u64).to_le_bytes());
        wide.extend_from_slice(&(data.len() as u64).to_le_bytes());
        wide.resize(zip64_len as usize, 0);
        out.extend_from_slice(&wide);
        let cd_size = out.len() - cd;
        w32(&mut out, SIG_EOCD);
        for v in [0u16, 0, 1, 1] {
            w16(&mut out, v);
        }
        w32(&mut out, cd_size as u32);
        w32(&mut out, cd as u32);
        w16(&mut out, 0);
        out
    }

    #[test]
    fn zip64_entry_sizes_are_read_from_the_extra_block() {
        let bytes = zip64_sized_entry(b"zip64 payload", 16);
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.entries[0].uncompressed_size(), 13);
        assert_eq!(a.read_name("x.bin").unwrap().unwrap(), b"zip64 payload");
        // A block too short for the fields it must carry is refused.
        assert!(matches!(Archive::parse(&zip64_sized_entry(b"zip64 payload", 8)), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn never_panics_on_truncation_or_corruption() {
        let mut w = Writer::new();
        w.add("a.txt", b"hello", Store::Stored { align: 64 }).unwrap();
        w.add("b.bin", &[7u8; 300], Store::Deflated).unwrap();
        for bytes in [w.finish(), with_zip64_end(two_entries(), true), zip64_sized_entry(b"zip64 payload", 16)] {
            for cut in 0..bytes.len() {
                if let Ok(a) = Archive::parse(&bytes[..cut]) {
                    for e in &a.entries {
                        let _ = a.read(e);
                    }
                }
            }
            for i in 0..bytes.len() {
                let mut v = bytes.clone();
                v[i] ^= 0x5A;
                if let Ok(a) = Archive::parse(&v) {
                    for e in &a.entries {
                        let _ = a.read(e);
                    }
                }
            }
        }
    }
}
