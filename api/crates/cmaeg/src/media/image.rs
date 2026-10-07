//! Bounded image normalization: what an agent is shown in place of an
//! inbound image.
//!
//! The pinned agent stack accepts only small inline images, so every image is
//! downscaled to a budget and re-encoded. The order of the pipeline is what
//! bounds memory: a full-size source is decoded once, under explicit limits,
//! and immediately reduced with a sampler that allocates only its output.
//! Rotation, alpha inspection and encoding then run on the small copy, never
//! on the source, because rotating or filtering a full-size source would
//! allocate another full-size (or `f32`) buffer. Re-encoding writes no
//! metadata, so EXIF (GPS included), XMP and ICC never leave the gateway.
//!
//! Every bound below is a test-enforced invariant: `tests/media_image_peak.rs`
//! decodes worst-case fixtures of every supported format under a counting
//! allocator and fails when the peak, the output or the wall time is exceeded.
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
pub const MAX_PIXELS: u64 = 24_000_000;
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
    // The header alone decides whether the pixels may be decoded at all.
    let mut header = header(input, format)?;
    let (width, height) = header.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS
        || width.max(height) > MAX_SIDE
        || header.total_bytes() > MAX_ALLOC
    {
        return Err(ImageError::TooLargePixels);
    }
    let orientation = header.orientation().unwrap_or(Orientation::NoTransforms);
    drop(header);
    // The pixels are decoded through `image`'s own non-generic, non-inlined
    // entry point, so the decoders are monomorphized where the codecs are
    // compiled instead of in this crate. Generic decoder code instantiated
    // here would run unoptimized in test builds, and the test-enforced wall
    // time would measure the build profile rather than the decoder. That entry
    // point guesses the format itself, so the guess must agree with the sniff;
    // its default limits sit on top of the stricter checks above. GIF and
    // WebP yield their first frame.
    if ::image::guess_format(input).ok() != Some(format) {
        return Err(ImageError::UnsupportedFormat);
    }
    let source = ::image::load_from_memory(input).map_err(classify)?;
    // Moving the source in drops it as soon as the small copy exists.
    let mut image = shrink(source, budget.max_side);
    image.apply_orientation(orientation);
    encode_within(image, budget)
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

/// A decoder of `input` that has read only its headers, under the decode
/// limits, so an oversized side is refused before anything else is parsed.
fn header(input: &[u8], format: ImageFormat) -> Result<impl ImageDecoder + '_, ImageError> {
    let mut reader = ImageReader::with_format(Cursor::new(input), format);
    // `Limits` is non-exhaustive, so it cannot be built as a literal.
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    reader.into_decoder().map_err(classify)
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
