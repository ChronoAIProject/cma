//! WebP fixtures.
//!
//! `image` writes only lossless WebP, so the two lossy samples are checked
//! in. cwebp 1.6.0 wrote them with `-q 80 -metadata none` from the 64x48
//! gradient that `image_fixtures::rgba(64, 48, 0, transparent)` draws:
//! [`LOSSY`] from the opaque one, as a simple-format `VP8 ` image, and
//! [`LOSSY_ALPHA`] from the transparent one, as an extended image whose
//! `ALPH` chunk libwebp compressed losslessly, its default. Everything else
//! is assembled from their chunks and from `image`'s lossless output.

pub const LOSSY: &[u8] = include_bytes!("../fixtures/webp/gradient_lossy.webp");
pub const LOSSY_ALPHA: &[u8] = include_bytes!("../fixtures/webp/gradient_lossy_alpha.webp");
/// The size of both samples.
pub const WIDTH: u32 = 64;
pub const HEIGHT: u32 = 48;

/// The `VP8X` flag of an image with transparency.
pub const ALPHA: u8 = 0b0001_0000;
/// The `VP8X` flag of an animation.
pub const ANIMATION: u8 = 0b0000_0010;
/// The `ALPH` header byte of raw alpha: no preprocessing, no filtering and
/// no compression.
pub const ALPHA_RAW: u8 = 0;

/// A chunk: its type, its little-endian length and its data, padded to an
/// even length as RIFF requires.
pub fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// A WebP holding `chunks`, its RIFF size counting all of them.
pub fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&u32::try_from(body.len() + 4).unwrap().to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

/// The first top-level chunk of `kind` in `webp`, a well-formed fixture,
/// with its header.
pub fn find_chunk(webp: &[u8], kind: &[u8; 4]) -> Vec<u8> {
    let mut offset = 12;
    while offset + 8 <= webp.len() {
        let length = u32::from_le_bytes(webp[offset + 4..offset + 8].try_into().unwrap());
        let end = offset + 8 + usize::try_from(length).unwrap();
        let padded = end + end % 2;
        if &webp[offset..offset + 4] == kind {
            return webp[offset..padded.min(webp.len())].to_vec();
        }
        offset = padded;
    }
    panic!("no {} chunk", String::from_utf8_lossy(kind));
}

/// A `VP8X` chunk with `flags` for a `width` by `height` canvas.
pub fn extended(flags: u8, width: u32, height: u32) -> Vec<u8> {
    let mut data = vec![flags, 0, 0, 0];
    data.extend_from_slice(&u24(width - 1));
    data.extend_from_slice(&u24(height - 1));
    chunk(b"VP8X", &data)
}

/// An `ALPH` chunk of raw alpha: the header byte, then one byte a pixel.
pub fn raw_alpha(alpha: &[u8]) -> Vec<u8> {
    let mut data = vec![ALPHA_RAW];
    data.extend_from_slice(alpha);
    chunk(b"ALPH", &data)
}

/// An `ANMF` chunk: a `width` by `height` frame at the canvas origin, not
/// blended, holding `chunks`.
pub fn frame(width: u32, height: u32, chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&u24(0));
    data.extend_from_slice(&u24(0));
    data.extend_from_slice(&u24(width - 1));
    data.extend_from_slice(&u24(height - 1));
    data.extend_from_slice(&u24(100));
    data.push(0b10);
    data.extend_from_slice(&chunks.concat());
    chunk(b"ANMF", &data)
}

/// An animation on a `width` by `height` canvas with the `VP8X` `flags`
/// besides [`ANIMATION`] and `frames`, each an `ANMF` chunk.
pub fn animation(flags: u8, width: u32, height: u32, frames: &[Vec<u8>]) -> Vec<u8> {
    let mut chunks = vec![
        extended(flags | ANIMATION, width, height),
        // A transparent background, looping forever.
        chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]),
    ];
    chunks.extend_from_slice(frames);
    riff(&chunks)
}

/// A frame of [`LOSSY`]'s image.
pub fn lossy_frame() -> Vec<u8> {
    frame(WIDTH, HEIGHT, &[find_chunk(LOSSY, b"VP8 ")])
}

/// A frame of [`LOSSY_ALPHA`]'s image, with its lossless alpha.
pub fn lossy_alpha_frame() -> Vec<u8> {
    frame(
        WIDTH,
        HEIGHT,
        &[
            find_chunk(LOSSY_ALPHA, b"ALPH"),
            find_chunk(LOSSY_ALPHA, b"VP8 "),
        ],
    )
}

/// A frame holding the `VP8L` chunk of `lossless`, a WebP `image` wrote.
pub fn lossless_frame(lossless: &[u8], width: u32, height: u32) -> Vec<u8> {
    frame(width, height, &[find_chunk(lossless, b"VP8L")])
}

fn u24(value: u32) -> [u8; 3] {
    let [low, middle, high, rest] = value.to_le_bytes();
    assert_eq!(rest, 0, "{value} does not fit in 24 bits");
    [low, middle, high]
}
