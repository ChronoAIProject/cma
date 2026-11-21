//! Structural MP4/3GPP eligibility, never full compressed-media decoding.
use super::bmff::{aac, atoms, child, one, sample_ranges, u32_at, Atom};
#[path = "video/bits.rs"]
mod bits;
#[path = "video/container.rs"]
mod container;
#[path = "video/h264.rs"]
mod h264;
#[path = "video/parameters.rs"]
mod parameters;

pub(crate) const MAX_WIDTH: u32 = 1920;
pub(crate) const MAX_HEIGHT: u32 = 1080;
pub(crate) const MAX_FRAME_RATE: u32 = 60;
pub(crate) const MAX_DURATION_MS: u32 = 600_000;
pub(crate) const MAX_SAMPLES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Mp4,
    ThreeGpp,
}
impl Format {
    pub(crate) fn mime(self) -> &'static str {
        match self {
            Self::Mp4 => "video/mp4",
            Self::ThreeGpp => "video/3gpp",
        }
    }
    pub(crate) fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Mp4 => &["mp4"],
            Self::ThreeGpp => &["3gp"],
        }
    }
}
pub(crate) fn inspect(bytes: &[u8]) -> Option<Format> {
    inspect_profile(bytes, true)
}

/// Telegram animations require an MP4 without an audio track.
pub(crate) fn silent_mp4(bytes: &[u8]) -> bool {
    inspect_profile(bytes, false) == Some(Format::Mp4)
}

fn inspect_profile(bytes: &[u8], allow_audio: bool) -> Option<Format> {
    if bytes.len() as u64 > crate::media::outbound::MAX_OUTBOUND_BYTES {
        return None;
    }
    let top = atoms(bytes, 0)?;
    container::only(&top, &[*b"ftyp", *b"moov", *b"mdat", *b"free"])?;
    let brand = one(&top, b"ftyp")?;
    if top.first()?.kind != *b"ftyp" || brand.body.len() < 8 || brand.body.len() % 4 != 0 {
        return None;
    }
    let format = match brand.body.get(..4)? {
        b"isom" | b"iso2" | b"mp41" | b"mp42" | b"avc1" => Format::Mp4,
        b"3gp4" | b"3gp5" | b"3gp6" | b"3gp7" | b"3gp8" | b"3gp9" => Format::ThreeGpp,
        _ => return None,
    };
    let movie = one(&top, b"moov")?;
    let data = one(&top, b"mdat")?;
    if movie.body_offset >= data.body_offset || data.body.is_empty() {
        return None;
    }
    let movie = child(movie)?;
    container::only(&movie, &[*b"mvhd", *b"trak", *b"udta"])?;
    let timing = container::movie(one(&movie, b"mvhd")?)?;
    let tracks: Vec<_> = movie.iter().filter(|atom| atom.kind == *b"trak").collect();
    if !(1..=2).contains(&tracks.len()) {
        return None;
    }
    let mut video = false;
    let mut audio = false;
    let mut previous_id = None;
    let mut all_ranges = Vec::new();
    for atom in tracks {
        let track = container::track(atom, timing)?;
        if previous_id == Some(track.id) {
            return None;
        }
        previous_id = Some(track.id);
        let ranges = sample_ranges(&top, &track.table, MAX_SAMPLES)?;
        if all_ranges.len() + ranges.len() > MAX_SAMPLES {
            return None;
        }
        if track.kind == *b"vide" {
            if video {
                return None;
            }
            video = true;
            let avc = avc(
                one(&track.descriptions, b"avc1")?,
                track.width,
                track.height,
            )?;
            let idr = ranges
                .iter()
                .map(|range| avc.sample(bytes.get(range.clone())?))
                .collect::<Option<Vec<_>>>()?;
            if idr.first() != Some(&true) {
                return None;
            }
            container::sync_samples(&track.table, &idr)?;
        } else {
            if !allow_audio || audio || track.width != 0 || track.height != 0 {
                return None;
            }
            audio = true;
            aac::sample_entry(one(&track.descriptions, b"mp4a")?)?;
        }
        all_ranges.extend(ranges);
    }
    // Prove each byte is visited at most once across tracks, before accepting.
    all_ranges.sort_unstable_by_key(|range| range.start);
    if !video
        || all_ranges
            .windows(2)
            .any(|pair| pair[0].end > pair[1].start)
    {
        return None;
    }
    Some(format)
}
fn avc(entry: &Atom<'_>, width: u32, height: u32) -> Option<h264::Avc> {
    let header = entry.body.get(..78)?;
    if header[..6] != [0; 6]
        || header[6..8] != [0, 1]
        || header[8..24] != [0; 16]
        || u32::from(u16::from_be_bytes(header[24..26].try_into().ok()?)) != width
        || u32::from(u16::from_be_bytes(header[26..28].try_into().ok()?)) != height
        || header[36..40] != [0; 4]
        || header[40..42] != [0, 1]
        || header[74..78] != [0, 24, 255, 255]
    {
        return None;
    }
    let extra = atoms(entry.body.get(78..)?, entry.body_offset + 78)?;
    container::only(&extra, &[*b"avcC", *b"btrt", *b"pasp", *b"colr"])?;
    if extra.iter().any(|atom| atom.kind == *b"pasp") {
        let aspect = one(&extra, b"pasp")?.body;
        if aspect.len() != 8 || u32_at(aspect, 0)? == 0 || u32_at(aspect, 0)? != u32_at(aspect, 4)?
        {
            return None;
        }
    }
    h264::Avc::parse(one(&extra, b"avcC")?.body, width, height)
}

#[cfg(test)]
#[path = "video_tests.rs"]
pub(crate) mod tests;
