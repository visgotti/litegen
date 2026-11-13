//! What a finished video is, read from its bytes (PixelGenie L10): its
//! length, frame rate, size and whether it carries audio — so a caller that
//! extracts frames knows what it got without probing the file itself. Reads
//! MP4 / MOV (ISO-BMFF boxes) and GIF; anything else reports nothing.

use crate::types::VideoMedia;

/// Everything the bytes say; a field litegen cannot read stays `None`.
pub fn probe(bytes: &[u8]) -> VideoMedia {
    if bytes.starts_with(b"GIF8") {
        return gif(std::io::Cursor::new(bytes)).unwrap_or_default();
    }
    mp4(bytes).unwrap_or_default()
}

/// The most of an MP4's `moov` box [`probe_file`] reads into memory: far
/// more than any real one (its sample tables), and still a bounded read.
const MAX_MOOV_BYTES: u64 = 64 * 1024 * 1024;

/// What the video file at `path` says, as [`probe`] would of its bytes,
/// without reading the file into memory: an MP4/MOV's top-level boxes are
/// walked by seeking, and only its `moov` is read (wherever it sits, before
/// or after the media); a GIF is decoded a frame at a time. Blocking: run it
/// on a blocking thread.
pub fn probe_file(path: &std::path::Path) -> VideoMedia {
    probe_path(path).unwrap_or_default()
}

fn probe_path(path: &std::path::Path) -> Option<VideoMedia> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = [0u8; 16];
    let mut at = 0u64;
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at)).ok()?;
        let got = file.by_ref().take(16).read(&mut head).ok()?;
        if at == 0 && head.starts_with(b"GIF8") {
            file.seek(SeekFrom::Start(0)).ok()?;
            return gif(std::io::BufReader::new(file));
        }
        if got < 8 {
            return None;
        }
        let (header, size) = match u32::from_be_bytes(head[..4].try_into().ok()?) {
            0 => (8, len - at),
            1 if got >= 16 => (16, u64::from_be_bytes(head[8..16].try_into().ok()?)),
            1 => return None,
            n => (8, u64::from(n)),
        };
        if size < header || size > len - at {
            return None;
        }
        if &head[4..8] == b"moov" {
            if size > MAX_MOOV_BYTES {
                return None;
            }
            let mut moov = vec![0u8; usize::try_from(size).ok()?];
            file.seek(SeekFrom::Start(at)).ok()?;
            file.read_exact(&mut moov).ok()?;
            return mp4(&moov);
        }
        at += size;
    }
    None
}

fn gif<R: std::io::BufRead + std::io::Seek>(reader: R) -> Option<VideoMedia> {
    use image::{AnimationDecoder, ImageDecoder};
    let decoder = image::codecs::gif::GifDecoder::new(reader).ok()?;
    let (width, height) = decoder.dimensions();
    let (mut frames, mut ms) = (0u32, 0f64);
    for frame in decoder.into_frames() {
        let frame = frame.ok()?;
        let (n, d) = frame.delay().numer_denom_ms();
        frames += 1;
        ms += f64::from(n) / f64::from(d.max(1));
    }
    let seconds = ms / 1000.0;
    Some(VideoMedia {
        duration_s: Some(seconds),
        fps: (seconds > 0.0).then(|| f64::from(frames) / seconds),
        width: Some(width),
        height: Some(height),
        has_audio: Some(false),
    })
}

/// The boxes directly inside `data`: (type, payload).
fn boxes(mut data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    std::iter::from_fn(move || {
        if data.len() < 8 {
            return None;
        }
        let size = u32::from_be_bytes(data[..4].try_into().ok()?) as u64;
        let kind = &data[4..8];
        let (header, size) = match size {
            0 => (8, data.len() as u64),
            1 if data.len() >= 16 => (16, u64::from_be_bytes(data[8..16].try_into().ok()?)),
            _ => (8, size),
        };
        let size = usize::try_from(size).ok().filter(|s| *s >= header && *s <= data.len())?;
        let payload = &data[header..size];
        data = &data[size..];
        Some((kind, payload))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| k == kind).map(|(_, p)| p)
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// (timescale, duration) of an `mvhd` or `mdhd` payload, by its version.
fn timing(header: &[u8]) -> Option<(u32, u64)> {
    match header.first()? {
        1 => Some((be32(header, 20)?, be64(header, 24)?)),
        _ => Some((be32(header, 12)?, u64::from(be32(header, 16)?))),
    }
}

fn mp4(bytes: &[u8]) -> Option<VideoMedia> {
    let moov = child(bytes, b"moov")?;
    let (timescale, duration) = timing(child(moov, b"mvhd")?)?;
    let mut media = VideoMedia {
        duration_s: (timescale > 0).then(|| duration as f64 / f64::from(timescale)),
        has_audio: Some(false),
        ..Default::default()
    };
    for (_, trak) in boxes(moov).filter(|(k, _)| *k == b"trak") {
        let Some(mdia) = child(trak, b"mdia") else { continue };
        let Some(hdlr) = child(mdia, b"hdlr") else { continue };
        match hdlr.get(8..12) {
            Some(b"soun") => media.has_audio = Some(true),
            Some(b"vide") if media.width.is_none() => {
                if let Some(tkhd) = child(trak, b"tkhd") {
                    // 16.16 fixed-point width and height, after the matrix.
                    let at = if tkhd.first() == Some(&1) { 88 } else { 76 };
                    media.width = be32(tkhd, at).map(|w| w >> 16);
                    media.height = be32(tkhd, at + 4).map(|h| h >> 16);
                }
                let stts = child(mdia, b"minf").and_then(|m| child(m, b"stbl")).and_then(|s| child(s, b"stts"));
                if let (Some((ts, dur)), Some(stts)) = (child(mdia, b"mdhd").and_then(timing), stts) {
                    let entries = be32(stts, 4).unwrap_or(0) as usize;
                    let frames: u64 = (0..entries).filter_map(|i| be32(stts, 8 + i * 8)).map(u64::from).sum();
                    let seconds = dur as f64 / f64::from(ts.max(1));
                    media.fps = (seconds > 0.0 && frames > 0).then(|| frames as f64 / seconds);
                }
            }
            _ => {}
        }
    }
    Some(media)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn full(version: u8, rest: &[u8]) -> Vec<u8> {
        let mut out = vec![version, 0, 0, 0];
        out.extend_from_slice(rest);
        out
    }

    fn header(timescale: u32, duration: u32) -> Vec<u8> {
        let mut rest = vec![0; 8];
        rest.extend_from_slice(&timescale.to_be_bytes());
        rest.extend_from_slice(&duration.to_be_bytes());
        rest.extend_from_slice(&[0; 80]);
        full(0, &rest)
    }

    fn trak(handler: &[u8; 4], size: Option<(u32, u32)>, timescale: u32, duration: u32, frames: Option<(u32, u32)>) -> Vec<u8> {
        let mut tkhd = vec![0; 72];
        let (w, h) = size.unwrap_or((0, 0));
        tkhd.extend_from_slice(&(w << 16).to_be_bytes());
        tkhd.extend_from_slice(&(h << 16).to_be_bytes());
        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0; 12]);
        let mut stbl = Vec::new();
        if let Some((count, delta)) = frames {
            let mut stts = 1u32.to_be_bytes().to_vec();
            stts.extend_from_slice(&count.to_be_bytes());
            stts.extend_from_slice(&delta.to_be_bytes());
            stbl = bx(b"stbl", &bx(b"stts", &full(0, &stts)));
        }
        let mdia = [bx(b"mdhd", &header(timescale, duration)), bx(b"hdlr", &full(0, &hdlr)), bx(b"minf", &stbl)].concat();
        bx(b"trak", &[bx(b"tkhd", &full(0, &tkhd)), bx(b"mdia", &mdia)].concat())
    }

    #[test]
    fn an_mp4_says_its_length_rate_size_and_audio() {
        // 2 s at 24 fps, 640×360, with an audio track.
        let moov = [
            bx(b"mvhd", &header(1000, 2000)),
            trak(b"vide", Some((640, 360)), 12_288, 24_576, Some((48, 512))),
            trak(b"soun", None, 48_000, 96_000, None),
        ]
        .concat();
        let file = [bx(b"ftyp", b"isomiso2"), bx(b"moov", &moov), bx(b"mdat", &[0; 16])].concat();
        assert_eq!(
            probe(&file),
            VideoMedia { duration_s: Some(2.0), fps: Some(24.0), width: Some(640), height: Some(360), has_audio: Some(true) }
        );
        let silent = [bx(b"moov", &[bx(b"mvhd", &header(1000, 2000)), trak(b"vide", Some((64, 64)), 10, 20, Some((20, 1)))].concat())].concat();
        assert_eq!(probe(&silent).has_audio, Some(false));
        assert_eq!(probe(&silent).fps, Some(10.0));
    }

    /// Review 2026-11-11 P4: a file probes as its bytes do, an MP4 whose
    /// `moov` comes after its media included.
    #[test]
    fn a_file_probes_as_its_bytes_do() {
        let moov = bx(b"moov", &[bx(b"mvhd", &header(1000, 2000)), trak(b"vide", Some((640, 360)), 12_288, 24_576, Some((48, 512))), trak(b"soun", None, 48_000, 96_000, None)].concat());
        let fast_start = [bx(b"ftyp", b"isomiso2"), moov.clone(), bx(b"mdat", &[0; 4096])].concat();
        let moov_last = [bx(b"ftyp", b"isomiso2"), bx(b"mdat", &[0; 4096]), moov].concat();
        let gif = crate::providers::video::visual_mock::generate_sprite_gif(2.0, 4, true);
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [("fast.mp4", &fast_start), ("last.mp4", &moov_last), ("clip.gif", &gif)] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(probe_file(&path), probe(bytes), "{name}");
            assert_ne!(probe_file(&path), VideoMedia::default(), "{name}: something was read");
        }
        assert_eq!(probe_file(&dir.path().join("last.mp4")).duration_s, Some(2.0));
        let junk = dir.path().join("junk.bin");
        std::fs::write(&junk, b"not a video").unwrap();
        assert_eq!(probe_file(&junk), VideoMedia::default());
        assert_eq!(probe_file(&dir.path().join("missing.mp4")), VideoMedia::default());
    }

    #[test]
    fn bytes_litegen_cannot_read_report_nothing() {
        assert_eq!(probe(b"not a video"), VideoMedia::default());
    }
}
