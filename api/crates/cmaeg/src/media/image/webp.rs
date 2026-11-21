//! Which WebP inputs would reach the lossless decoder.
//!
//! `image-webp`'s lossless decoder has no memory bound. Its memory limit
//! covers only the ICC, EXIF and XMP chunk reads, `image` passes it no
//! allocation limit, and the lossless bitstream itself sizes what it
//! allocates, such as its Huffman groups, so the header checks cannot bound
//! it. The decoder reads two kinds of chunk: a `VP8L` image bitstream and an
//! `ALPH` alpha plane whose compression is lossless. Either may sit at the top
//! level or inside an `ANMF` animation frame. Such a WebP is refused before any
//! decoder reads it.
//!
//! Lossless alpha is how libwebp stores the transparency of a lossy WebP by
//! default, so refusing it also refuses most transparent lossy WebPs, such as
//! Telegram stickers. Stripping that alpha instead would decode them, but
//! shows whatever colours the encoder left under the transparent pixels:
//! libwebp rewrites them to compress better, so the agent would be shown a
//! background the sender never saw. A raw (uncompressed) alpha plane is
//! decoded with a single allocation of the frame's size, which the header
//! checks bound, and is accepted.
//!
//! The walk covers more than the decoder can reach. It ignores the RIFF
//! size, which the decoder reads past, and visits every top-level chunk and
//! every chunk of every frame, although `image` decodes only the first
//! frame. It clamps every length to the input, so it cannot panic on any
//! input: a truncated chunk ends the walk, and the decoder then reports the
//! truncation itself.

/// The RIFF signature, its size and the `WEBP` form type before the first
/// chunk.
const RIFF_HEADER: usize = 12;
/// A chunk's type and its little-endian length, which precede its data.
const CHUNK_HEADER: usize = 8;
/// An `ANMF` chunk's position, size, duration and flags, which precede the
/// frame's own chunks.
const FRAME_HEADER: usize = 16;
/// The bits of an `ALPH` chunk's first byte that name its compression.
const ALPHA_COMPRESSION: u8 = 0b11;
/// The `ALPH` compression read by the lossless decoder.
const ALPHA_LOSSLESS: u8 = 1;

/// A chunk that only the lossless decoder reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LosslessChunk {
    /// A top-level `VP8L` image.
    Image,
    /// A top-level `ALPH` with lossless compression.
    Alpha,
    /// A `VP8L` inside an animation frame.
    FrameImage,
    /// An `ALPH` with lossless compression inside an animation frame.
    FrameAlpha,
}

impl LosslessChunk {
    /// A stable name for logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Image => "vp8l",
            Self::Alpha => "alph",
            Self::FrameImage => "anmf_vp8l",
            Self::FrameAlpha => "anmf_alph",
        }
    }
}

/// The first chunk of `input`, a WebP, that the lossless decoder would read,
/// if any.
pub fn lossless_chunk(input: &[u8]) -> Option<LosslessChunk> {
    for chunk in Chunks::new(input, RIFF_HEADER, input.len()) {
        match &chunk.kind {
            b"VP8L" => return Some(LosslessChunk::Image),
            b"ALPH" if is_lossless_alpha(chunk.data) => return Some(LosslessChunk::Alpha),
            b"ANMF" => {
                let frame = Chunks::new(
                    input,
                    chunk.start.saturating_add(FRAME_HEADER),
                    chunk.start + chunk.data.len(),
                );
                for inner in frame {
                    match &inner.kind {
                        b"VP8L" => return Some(LosslessChunk::FrameImage),
                        b"ALPH" if is_lossless_alpha(inner.data) => {
                            return Some(LosslessChunk::FrameAlpha)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether an `ALPH` chunk's data names lossless compression. Empty data
/// names nothing, and the decoder refuses it.
fn is_lossless_alpha(data: &[u8]) -> bool {
    data.first()
        .is_some_and(|info| info & ALPHA_COMPRESSION == ALPHA_LOSSLESS)
}

struct Chunk<'a> {
    kind: [u8; 4],
    /// Where the data starts in the whole input.
    start: usize,
    /// The data, clamped to the end of the input.
    data: &'a [u8],
}

/// The chunks whose headers lie in `offset..end` of `input`.
///
/// Only the headers are bounded by `end`: a frame's chunk may declare data
/// past the end of its frame, and the decoder reads it from the input, so
/// data is clamped to the input alone.
struct Chunks<'a> {
    input: &'a [u8],
    offset: usize,
    end: usize,
}

impl<'a> Chunks<'a> {
    fn new(input: &'a [u8], offset: usize, end: usize) -> Self {
        Self {
            input,
            offset,
            end: end.min(input.len()),
        }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Chunk<'a>> {
        let start = self.offset.checked_add(CHUNK_HEADER)?;
        if start > self.end {
            return None;
        }
        let head = self.input.get(self.offset..start)?;
        let kind = [head[0], head[1], head[2], head[3]];
        let length = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        let declared = usize::try_from(length).unwrap_or(usize::MAX);
        let data_end = start.saturating_add(declared).min(self.input.len());
        // RIFF pads a chunk of odd length with one byte that its length does
        // not count.
        self.offset = start
            .saturating_add(declared)
            .saturating_add(usize::from(length % 2 == 1));
        Some(Chunk {
            kind,
            start,
            data: &self.input[start..data_end],
        })
    }
}
