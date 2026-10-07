//! Image fixtures built in code, shared by the functional image tests and the
//! peak-memory test binary.
use image::{
    codecs::{
        gif::GifEncoder,
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
        webp::WebPEncoder,
    },
    Delay, ExtendedColorType, Frame, ImageEncoder, RgbImage, RgbaImage,
};

/// A cheap deterministic generator, so fixtures are reproducible.
pub struct Noise(u64);

impl Noise {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn byte(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 24) as u8
    }
}

/// A gradient with `noise` amplitude of per-pixel noise: realistic detail at
/// small amplitudes, incompressible at large ones.
pub fn rgb(width: u32, height: u32, noise: u8) -> RgbImage {
    let mut random = Noise::new(u64::from(width) * 31 + u64::from(height));
    let mut buffer = vec![0_u8; width as usize * height as usize * 3];
    for (index, pixel) in buffer.chunks_exact_mut(3).enumerate() {
        let x = (index % width as usize) as u32;
        let y = (index / width as usize) as u32;
        let jitter = |random: &mut Noise| {
            if noise == 0 {
                0
            } else {
                random.byte() % noise
            }
        };
        pixel[0] = ((x * 255 / width) as u8).wrapping_add(jitter(&mut random));
        pixel[1] = ((y * 255 / height) as u8).wrapping_add(jitter(&mut random));
        pixel[2] = (((x + y) * 127 / (width + height)) as u8).wrapping_add(jitter(&mut random));
    }
    RgbImage::from_raw(width, height, buffer).unwrap()
}

/// `rgb` with an alpha channel: opaque unless `transparent`, in which case
/// the left half fades out.
pub fn rgba(width: u32, height: u32, noise: u8, transparent: bool) -> RgbaImage {
    let source = rgb(width, height, noise);
    let mut buffer = Vec::with_capacity(width as usize * height as usize * 4);
    for (index, pixel) in source.as_raw().chunks_exact(3).enumerate() {
        let x = (index % width as usize) as u32;
        let alpha = if transparent && x < width / 2 {
            (x * 255 / (width / 2).max(1)) as u8
        } else {
            u8::MAX
        };
        buffer.extend_from_slice(&[pixel[0], pixel[1], pixel[2], alpha]);
    }
    RgbaImage::from_raw(width, height, buffer).unwrap()
}

pub fn jpeg(image: &RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(image)
        .unwrap();
    out
}

/// A progressive 4:4:4 JPEG, which `image` cannot write.
pub fn progressive_jpeg(image: &RgbImage, quality: u8) -> Vec<u8> {
    progressive(
        image.as_raw(),
        image.width(),
        image.height(),
        jpeg_encoder::ColorType::Rgb,
        quality,
    )
}

/// A progressive 4:4:4 CMYK JPEG: four full-resolution components, the
/// most coefficient storage a progressive decode can need. `noise` sets its
/// size as in [`rgb`], and the key channel is noise alone.
pub fn progressive_cmyk_jpeg(width: u32, height: u32, noise: u8, quality: u8) -> Vec<u8> {
    let source = rgb(width, height, noise);
    let mut random = Noise::new(u64::from(width) + u64::from(height) * 31);
    let mut samples = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in source.as_raw().chunks_exact(3) {
        let key = if noise == 0 { 0 } else { random.byte() % noise };
        samples.extend_from_slice(&[pixel[0], pixel[1], pixel[2], key]);
    }
    progressive(
        &samples,
        width,
        height,
        jpeg_encoder::ColorType::Cmyk,
        quality,
    )
}

fn progressive(
    samples: &[u8],
    width: u32,
    height: u32,
    color: jpeg_encoder::ColorType,
    quality: u8,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    encoder.set_progressive(true);
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    encoder
        .encode(
            samples,
            u16::try_from(width).unwrap(),
            u16::try_from(height).unwrap(),
            color,
        )
        .unwrap();
    out
}

/// `jpeg` with an EXIF APP1 segment carrying `orientation` spliced in
/// directly after the SOI marker.
pub fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let mut exif = b"Exif\0\0".to_vec();
    // Big-endian TIFF header, then one IFD with one SHORT entry.
    exif.extend_from_slice(b"MM\0\x2A\0\0\0\x08");
    exif.extend_from_slice(&1_u16.to_be_bytes());
    exif.extend_from_slice(&0x0112_u16.to_be_bytes());
    exif.extend_from_slice(&3_u16.to_be_bytes());
    exif.extend_from_slice(&1_u32.to_be_bytes());
    exif.extend_from_slice(&orientation.to_be_bytes());
    exif.extend_from_slice(&[0, 0]);
    exif.extend_from_slice(&0_u32.to_be_bytes());
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
    out.extend_from_slice(&exif);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// `jpeg` with ICC profile segments spliced in after the SOI marker, as
/// many whole segments as fit in `len` bytes. A JPEG decoder keeps a copy of
/// every segment, so this is the most a JPEG of `len` bytes makes it hold
/// beside its pixels.
pub fn with_icc_segments(jpeg: &[u8], len: usize) -> Vec<u8> {
    // A segment's length field counts itself, the identifier, the sequence
    // number and count bytes, and the data, and is at most 65535.
    const DATA: usize = 65_535 - 2 - 12 - 2;
    const SEGMENT: usize = 2 + 65_535;
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let count = len.saturating_sub(jpeg.len()) / SEGMENT;
    let mut out = Vec::with_capacity(jpeg.len() + count * SEGMENT);
    out.extend_from_slice(&jpeg[..2]);
    for index in 0..count {
        out.extend_from_slice(&[0xFF, 0xE2]);
        out.extend_from_slice(&65_535_u16.to_be_bytes());
        out.extend_from_slice(b"ICC_PROFILE\0");
        // Sequence numbers are one byte; the decoder keeps every segment
        // whatever its number.
        out.push(u8::try_from(index % 255 + 1).unwrap());
        out.push(u8::MAX);
        out.resize(out.len() + DATA, 0x5A);
    }
    out.extend_from_slice(&jpeg[2..]);
    out
}

pub fn png_rgba8(image: &RgbaImage) -> Vec<u8> {
    png(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )
}

pub fn png_rgb8(image: &RgbImage) -> Vec<u8> {
    png(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgb8,
    )
}

/// A 16-bit RGBA PNG; the encoder takes big-endian samples.
pub fn png_rgba16(width: u32, height: u32) -> Vec<u8> {
    let source = rgba(width, height, 8, false);
    let mut samples = Vec::with_capacity(source.as_raw().len() * 2);
    for &sample in source.as_raw() {
        samples.extend_from_slice(&[sample, sample]);
    }
    png(&samples, width, height, ExtendedColorType::Rgba16)
}

fn png(data: &[u8], width: u32, height: u32, color: ExtendedColorType) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::NoFilter)
        .write_image(data, width, height, color)
        .unwrap();
    out
}

/// A GIF with one frame per colour, at the fastest quantizer speed. With
/// `noisy`, the red channel of every pixel varies through 256 levels, which
/// keeps each frame at exactly 256 colours and its LZW stream nearly
/// incompressible: the decoder's worst case.
pub fn gif(width: u32, height: u32, frames: &[[u8; 3]], noisy: bool) -> Vec<u8> {
    let mut random = Noise::new(7);
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut out, 30);
        for color in frames {
            let frame = RgbaImage::from_fn(width, height, |_, _| {
                let red = if noisy { random.byte() } else { color[0] };
                image::Rgba([red, color[1], color[2], u8::MAX])
            });
            encoder
                .encode_frame(Frame::from_parts(
                    frame,
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                ))
                .unwrap();
        }
    }
    out
}

/// A GIF whose logical screen is `screen` pixels square and whose only frame
/// claims `frame` pixels square, built by hand because an encoder always
/// fits the screen to its frames. The frame carries no pixels, only a clear
/// code and an end code, so a decoder that admits it allocates the frame's
/// buffer and then finds the image truncated.
pub fn gif_with_frame(screen: u16, frame: u16) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&screen.to_le_bytes());
    out.extend_from_slice(&screen.to_le_bytes());
    // A two-entry global colour table, black and white.
    out.extend_from_slice(&[0x80, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF]);
    // The image descriptor: at the origin, no local table, not interlaced.
    out.push(0x2C);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&frame.to_le_bytes());
    out.extend_from_slice(&frame.to_le_bytes());
    out.push(0);
    // LZW minimum code size 2, so 3-bit codes: clear (4) then end (5), packed
    // least significant bit first into one byte, in a one-byte sub-block.
    out.extend_from_slice(&[2, 1, 0b0010_1100, 0]);
    out.push(0x3B);
    out
}

pub fn webp_lossless(image: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .unwrap();
    out
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = kind.to_vec();
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// A PNG signature and an IHDR claiming `width` by `height`, with no pixel
/// data at all.
pub fn png_header_only(width: u32, height: u32) -> Vec<u8> {
    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&height.to_be_bytes());
    // 8-bit RGBA, deflate, adaptive filtering, no interlace.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&chunk(b"IHDR", &ihdr));
    out
}

/// [`png_header_only`] completed with an empty IDAT and IEND, so a decoder
/// reads its whole header and still finds no pixel data.
pub fn png_without_pixels(width: u32, height: u32) -> Vec<u8> {
    let mut out = png_header_only(width, height);
    out.extend_from_slice(&chunk(b"IDAT", &[]));
    out.extend_from_slice(&chunk(b"IEND", &[]));
    out
}

/// The PNG chunk CRC (ISO 3309), bit by bit: fixtures are tiny.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
