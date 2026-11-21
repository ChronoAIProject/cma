//! Bounded image normalization: what an agent is shown in place of an
//! inbound image.
//!
//! The pinned agent stack accepts only small inline images, so every image is
//! downscaled to a budget and re-encoded. The order of the pipeline is what
//! bounds memory: a header read under explicit limits admits a full-size
//! source, which is decoded once and immediately reduced with a sampler that
//! allocates only its output.
//! Rotation, alpha inspection and encoding then run on the small copy, never
//! on the source, because rotating or filtering a full-size source would
//! allocate another full-size (or `f32`) buffer. A PNG's ICC profile, the
//! one piece of metadata a decoder inflates, is removed before even the
//! header is read, and a WebP that needs the lossless decoder, which has no
//! memory bound at all, is refused there (`webp` says which). Re-encoding
//! writes no metadata, so EXIF (GPS included), XMP and ICC never leave the
//! gateway.
//!
//! Every bound below is a test-enforced invariant: `tests/media_image_peak.rs`
//! decodes worst-case fixtures of every supported format under a counting
//! allocator and fails when the peak, the output or the wall time is exceeded.
//!
//! Measured on an Apple M-series test build at these bounds, the costliest
//! input is a progressive 4:4:4 CMYK JPEG. Its decoder holds four planes of
//! 16-bit coefficients beside the RGB output, about 11 bytes a pixel, plus a
//! copy of the input and a copy of every metadata segment in it. Padded with
//! ICC segments to the 25 MiB `CMAEG_MEDIA_MAX_BYTES` ceiling, it peaks at
//! 237.8 MiB; with a 23.5 MiB noisy input, at 212.7 MiB. The other fixtures
//! peak at 62.3 MiB (baseline JPEG), 165.2 MiB (progressive RGB JPEG),
//! 76.7 MiB (RGBA8 PNG), 154.1 MiB (16-bit PNG), 85.8 MiB (two-frame GIF)
//! and 137.3 MiB (lossy WebP animation), and every fixture finishes within
//! 1.0 s. `image` cannot write lossy WebP, so the one at the limit is a small
//! checked-in frame on a full-size canvas, where WebP allocates the most: the
//! canvas and the output beside it. A still lossy WebP at the limit, written
//! by cwebp and measured by hand, peaked at 107.7 MiB in 1.7 s with random
//! noise, the slowest decode, and at 111.6 MiB with an uncompressed alpha
//! plane filling the input ceiling.
//! [`MAX_PIXELS`] was lowered from 24,000,000 for the CMYK JPEG, which there
//! peaked at 252 MiB plus its input, 277 MiB at the ceiling; at 20,000,000
//! the padded one still peaked at 258.9 MiB.
//!
//! A 512x512 PNG whose ICC profile would inflate to 511 MiB peaked at
//! 529.5 MiB before profiles were removed, and now at 2.8 MiB. Removing one
//! copies the input: beside the 6000x3000 RGBA8 PNG fixture, a 68.7 MiB input
//! well over the 25 MiB ceiling, the copy raises its peak to 145.6 MiB.
mod webp;

use super::{cache::Weighted, sniff::sniff};
use crate::telemetry::names;
use ::image::{
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
    },
    metadata::Orientation,
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
    RgbImage, RgbaImage,
};
use bytes::Bytes;
use std::{
    borrow::Cow,
    fmt,
    io::Cursor,
    sync::Arc,
    time::{Duration, Instant as StdInstant},
};
use tokio::{
    sync::Semaphore,
    time::{timeout_at, Instant},
};

/// Largest decoded image, in pixels. Test-enforced: every supported format at
/// this size decodes within [`DECODE_PEAK_BUDGET`] and [`DECODE_WALL_BUDGET`].
/// It admits an 18-megapixel camera's 5184x3456 but not 20 or 24 megapixels,
/// which as a progressive CMYK JPEG exceed the peak budget.
pub const MAX_PIXELS: u64 = 18_000_000;
/// Longest accepted side, in pixels. Test-enforced with [`MAX_PIXELS`].
pub const MAX_SIDE: u32 = 12_000;
/// The decoder allocation limit. `image` treats it as a hint some decoders
/// ignore, which is why [`MAX_PIXELS`] is checked from the header first.
pub const MAX_ALLOC: u64 = 128 * 1024 * 1024;
/// Peak bytes allocated by one normalization. Test-enforced.
pub const DECODE_PEAK_BUDGET: usize = 256 * 1024 * 1024;
/// Wall time of one normalization. Test-enforced; a blocking decode cannot be
/// cancelled, so this bound is what limits how long a decode permit is held.
pub const DECODE_WALL_BUDGET: Duration = Duration::from_secs(3);

/// The size an output must fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageBudget {
    pub max_side: u32,
    pub max_bytes: usize,
}

/// What a read result shows: 1 MiB of raw image bytes is about 1.4 MiB once
/// serialized, well inside the agent runtime's 4 MiB output frames.
pub const FULL: ImageBudget = ImageBudget {
    max_side: 2048,
    max_bytes: 1_048_576,
};
/// A preview: six fit in the budget of one [`FULL`] image.
pub const PREVIEW: ImageBudget = ImageBudget {
    max_side: 768,
    max_bytes: 163_840,
};

/// Smaller sizes and qualities tried, in order, when an output is over budget.
const STEP_DOWN: [(u32, u32, u8); 2] = [(3, 4, 75), (1, 2, 70)];
const JPEG_QUALITY: u8 = 85;
/// Enough for every supported format's signature.
const SNIFF_HEAD: usize = 64;
/// The PNG signature that precedes its first chunk.
const PNG_SIGNATURE_LEN: usize = 8;
/// A PNG chunk's length and type fields, which precede its data.
const PNG_CHUNK_HEAD: usize = 8;
/// A PNG chunk's length, type and checksum fields together.
const PNG_CHUNK_FRAMING: usize = 12;

/// A re-encoded image without metadata, within its budget.
pub struct NormalizedImage {
    pub bytes: Bytes,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    /// The frame's position in a video; `None` for images and thumbnails.
    pub at_ms: Option<u64>,
}

/// Content must never reach a log line, so only the size is shown.
impl fmt::Debug for NormalizedImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizedImage")
            .field("bytes", &self.bytes.len())
            .field("mime", &self.mime)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("at_ms", &self.at_ms)
            .finish()
    }
}

impl Weighted for NormalizedImage {
    fn weight(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// Why an image could not be normalized. No decoder message is kept: it can
/// quote input bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Not PNG, JPEG, GIF or WebP, or a feature of one the decoder lacks.
    UnsupportedFormat,
    /// Over [`MAX_PIXELS`] or [`MAX_SIDE`], or over the decoder's limits.
    TooLargePixels,
    DecodeFailed,
    /// Still over budget at the smallest step.
    TooLarge,
    /// The decode permit was not free before the deadline.
    TimedOut,
}

impl ImageError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "unsupported_format",
            Self::TooLargePixels => "too_large_pixels",
            Self::DecodeFailed => "decode_failed",
            Self::TooLarge => "too_large",
            Self::TimedOut => "timed_out",
        }
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for ImageError {}

/// Normalize `input` on the blocking pool under the single decode permit.
pub async fn normalize(
    input: Bytes,
    budget: ImageBudget,
    decode: &Arc<Semaphore>,
    deadline: Instant,
) -> Result<NormalizedImage, ImageError> {
    let started = StdInstant::now();
    let result = run(input, budget, decode, deadline).await;
    let outcome = result.as_ref().err().map_or("ok", ImageError::as_str);
    metrics::histogram!(
        names::MEDIA_DERIVE,
        names::OP => "image",
        names::OUTCOME => outcome
    )
    .record(started.elapsed().as_secs_f64());
    tracing::debug!(outcome, "image normalization finished");
    result
}

async fn run(
    input: Bytes,
    budget: ImageBudget,
    decode: &Arc<Semaphore>,
    deadline: Instant,
) -> Result<NormalizedImage, ImageError> {
    let permit = timeout_at(deadline, decode.clone().acquire_owned())
        .await
        .map_err(|_| ImageError::TimedOut)?
        .map_err(|_| ImageError::TimedOut)?;
    // A blocking task cannot be cancelled: when the awaiting future is
    // dropped, the decode still runs to completion. The task therefore owns
    // the permit, so the memory it uses stays counted until it really ends.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        normalize_blocking(&input, budget)
    })
    .await
    .map_err(|_| {
        tracing::error!("image normalization task panicked or was cancelled");
        ImageError::DecodeFailed
    })?
}

/// Decode, downscale, orient and re-encode `input` within `budget`.
pub fn normalize_blocking(
    input: &[u8],
    budget: ImageBudget,
) -> Result<NormalizedImage, ImageError> {
    let head = &input[..input.len().min(SNIFF_HEAD)];
    let format = supported(sniff(head).mime).ok_or(ImageError::UnsupportedFormat)?;
    let input = decodable(input, format)?;
    let input = input.as_ref();
    // The header alone decides whether the pixels may be decoded at all.
    let mut header = limited(input, format).into_decoder().map_err(classify)?;
    let (width, height) = header.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS
        || width.max(height) > MAX_SIDE
        || header.total_bytes() > MAX_ALLOC
    {
        return Err(ImageError::TooLargePixels);
    }
    let orientation = header.orientation().unwrap_or(Orientation::NoTransforms);
    drop(header);
    let source = pixels(input, format)?;
    // Moving the source in drops it as soon as the small copy exists.
    let mut image = shrink(source, budget.max_side);
    image.apply_orientation(orientation);
    encode_within(image, budget)
}

/// The pixels of `input`, whose header the checks above admitted. GIF and
/// WebP yield their first frame.
fn pixels(input: &[u8], format: ImageFormat) -> Result<DynamicImage, ImageError> {
    if format == ImageFormat::Gif {
        // A GIF's header describes only its logical screen. Its first frame
        // may be far larger, and unless it spans the screen's width from the
        // left edge it is decoded into a buffer of its own. `decode` charges
        // the screen to `MAX_ALLOC` first, so that buffer must fit in what is
        // left, and is refused as a limit otherwise.
        return limited(input, format).decode().map_err(classify);
    }
    // PNG, JPEG and lossy WebP decoders allocate what the header checks
    // bounded: their output, and planes or an animation canvas of the same
    // dimensions. Otherwise they copy metadata the input holds or allocate
    // buffers `image` does not charge to the limits at all, so the limits
    // would add nothing here. What would get around the header checks never
    // gets here: a PNG's ICC profile, the one piece of metadata a decoder
    // inflates, was removed before the header was read, and a WebP that needs
    // the lossless decoder, whose bitstream sizes its own allocations, was
    // refused there. Their pixels are decoded through `image`'s non-generic,
    // non-inlined entry point instead, so the decoders are monomorphized
    // where the codecs are compiled with optimizations rather than in this
    // crate. Instantiated here they run unoptimized in test builds, where a
    // 16-bit PNG took 4 s, and the test-enforced wall time would measure the
    // build profile rather than the decoder. That entry point guesses the
    // format itself, so the guess must agree with the sniff.
    if ::image::guess_format(input).ok() != Some(format) {
        return Err(ImageError::UnsupportedFormat);
    }
    ::image::load_from_memory(input).map_err(classify)
}

/// `input` as its decoders may read it: a PNG without its ICC profiles, and a
/// WebP only when no part of it needs the unbounded lossless decoder.
fn decodable(input: &[u8], format: ImageFormat) -> Result<Cow<'_, [u8]>, ImageError> {
    match format {
        ImageFormat::Png => Ok(without_icc_profiles(input)),
        ImageFormat::WebP => match webp::lossless_chunk(input) {
            None => Ok(Cow::Borrowed(input)),
            Some(chunk) => {
                tracing::debug!(
                    chunk = chunk.as_str(),
                    "refused a webp that only the lossless decoder reads"
                );
                Err(ImageError::UnsupportedFormat)
            }
        },
        _ => Ok(Cow::Borrowed(input)),
    }
}

/// `input`, a PNG, without its ICC profile chunks.
///
/// A PNG decoder inflates the profile while it reads the header, bounded only
/// by its allocation limit, and silently drops a profile over that limit
/// instead of refusing the image. A profile of a few hundred KiB therefore
/// inflates to whatever limit applies: 128 MiB for the header read and
/// 512 MiB through `load_from_memory`, twice [`DECODE_PEAK_BUDGET`]. Decoding
/// never applies the profile and re-encoding discards it, so its chunk is
/// removed before either read. Every chunk carries its own length and
/// checksum, so what remains is still a valid PNG. A decoder refuses a
/// profile after the image data without inflating it, so the walk ends at
/// the first image data chunk. A chunk whose length runs past the input ends
/// it too, and the decoder reports the truncation.
///
/// A copy is made only when a profile is present, and it is never larger
/// than the input.
fn without_icc_profiles(input: &[u8]) -> Cow<'_, [u8]> {
    let mut kept: Option<Vec<u8>> = None;
    let mut removed = 0_usize;
    // Everything before `copied` is already in `kept` or was removed.
    let mut copied = 0;
    let mut offset = PNG_SIGNATURE_LEN;
    while let Some(head) = input.get(offset..offset.saturating_add(PNG_CHUNK_HEAD)) {
        let (length, kind) = head.split_at(4);
        let length = u32::from_be_bytes([length[0], length[1], length[2], length[3]]);
        let end = usize::try_from(length)
            .unwrap_or(usize::MAX)
            .saturating_add(PNG_CHUNK_FRAMING)
            .saturating_add(offset)
            .min(input.len());
        match kind {
            b"IDAT" => break,
            b"iCCP" => {
                let out = kept.get_or_insert_with(|| Vec::with_capacity(input.len()));
                out.extend_from_slice(&input[copied..offset]);
                copied = end;
                removed += 1;
            }
            _ => {}
        }
        offset = end;
    }
    match kept {
        None => Cow::Borrowed(input),
        Some(mut out) => {
            out.extend_from_slice(&input[copied..]);
            tracing::debug!(removed, "removed embedded icc profiles before decoding");
            Cow::Owned(out)
        }
    }
}

fn supported(mime: &str) -> Option<ImageFormat> {
    match mime {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

/// A reader of `input` under the decode limits, so an oversized side is
/// refused while the header is read, before anything else is parsed.
fn limited(input: &[u8], format: ImageFormat) -> ImageReader<Cursor<&[u8]>> {
    let mut reader = ImageReader::with_format(Cursor::new(input), format);
    // `Limits` is non-exhaustive, so it cannot be built as a literal.
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    reader
}

fn classify(error: ::image::ImageError) -> ImageError {
    match error {
        ::image::ImageError::Limits(_) => ImageError::TooLargePixels,
        ::image::ImageError::Unsupported(_) => ImageError::UnsupportedFormat,
        _ => ImageError::DecodeFailed,
    }
}

/// `image` reduced so its long side is at most `max_side`, aspect preserved.
/// `thumbnail` allocates only its output; `resize` would allocate a
/// full-size `f32` intermediate.
fn shrink(image: DynamicImage, max_side: u32) -> DynamicImage {
    if image.width().max(image.height()) <= max_side {
        return image;
    }
    image.thumbnail(max_side, max_side)
}

/// Encode at the budget's side, then at each smaller step, until it fits.
fn encode_within(image: DynamicImage, budget: ImageBudget) -> Result<NormalizedImage, ImageError> {
    let mut image = image;
    let mut transparent = has_real_alpha(&image);
    let sides = [(budget.max_side, JPEG_QUALITY)]
        .into_iter()
        .chain(STEP_DOWN.iter().map(|&(numerator, denominator, quality)| {
            (budget.max_side * numerator / denominator, quality)
        }));
    for (side, quality) in sides {
        image = shrink(image, side);
        if transparent {
            let rgba = image.to_rgba8();
            let png = encode_png(&rgba)?;
            if png.len() <= budget.max_bytes {
                return Ok(normalized(png, "image/png", &rgba));
            }
            // Transparency is not worth an image that cannot be shown.
            image = DynamicImage::ImageRgb8(flatten_on_white(&rgba));
            transparent = false;
        }
        let rgb = image.to_rgb8();
        let jpeg = encode_jpeg(&rgb, quality)?;
        if jpeg.len() <= budget.max_bytes {
            return Ok(normalized(jpeg, "image/jpeg", &rgb));
        }
    }
    Err(ImageError::TooLarge)
}

fn normalized<P: ::image::Pixel>(
    bytes: Vec<u8>,
    mime: &'static str,
    image: &::image::ImageBuffer<P, Vec<P::Subpixel>>,
) -> NormalizedImage {
    NormalizedImage {
        bytes: Bytes::from(bytes),
        mime,
        width: image.width(),
        height: image.height(),
        at_ms: None,
    }
}

/// Whether any pixel is less than fully opaque, not merely whether the
/// format has an alpha channel: most "RGBA" screenshots are opaque, and a
/// JPEG of them is far smaller than a PNG.
fn has_real_alpha(image: &DynamicImage) -> bool {
    image.color().has_alpha() && image.to_rgba8().pixels().any(|pixel| pixel.0[3] < u8::MAX)
}

fn flatten_on_white(rgba: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [red, green, blue, alpha] = rgba.get_pixel(x, y).0;
        let blend = |channel: u8| -> u8 {
            let alpha = u16::from(alpha);
            let value = (u16::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255;
            u8::try_from(value).unwrap_or(u8::MAX)
        };
        ::image::Rgb([blend(red), blend(green), blend(blue)])
    })
}

fn encode_png(rgba: &RgbaImage) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Default, FilterType::Adaptive)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| ImageError::DecodeFailed)?;
    Ok(out)
}

fn encode_jpeg(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(rgb)
        .map_err(|_| ImageError::DecodeFailed)?;
    Ok(out)
}
