//! Image normalization: budgets, orientation, metadata stripping, format
//! handling and refusals.
use super::{image_fixtures as fixtures, webp_fixtures as webp};
use bytes::Bytes;
use cmaeg::media::image::{
    normalize, normalize_blocking, ImageError, NormalizedImage, FULL, MAX_PIXELS, PREVIEW,
};
use std::{
    sync::Arc,
    time::{Duration, Instant as StdInstant},
};
use tokio::{sync::Semaphore, time::Instant};

fn decoded(output: &NormalizedImage) -> image::DynamicImage {
    image::load_from_memory(&output.bytes).unwrap()
}

fn assert_within(output: &NormalizedImage, budget: cmaeg::media::image::ImageBudget) {
    assert!(output.bytes.len() <= budget.max_bytes, "{output:?}");
    assert!(
        output.width.max(output.height) <= budget.max_side,
        "{output:?}"
    );
    assert_eq!(output.at_ms, None);
    let image = decoded(output);
    assert_eq!(
        (image.width(), image.height()),
        (output.width, output.height)
    );
}

#[test]
fn a_large_photo_is_downscaled_to_the_full_budget() {
    let input = fixtures::jpeg(&fixtures::rgb(4000, 3000, 16), 90);
    let output = normalize_blocking(&input, FULL).unwrap();
    assert_eq!((output.width, output.height), (2048, 1536));
    assert_eq!(output.mime, "image/jpeg");
    assert_within(&output, FULL);
}

#[test]
fn exif_orientation_rotates_the_output_and_no_metadata_survives() {
    let landscape = fixtures::jpeg(&fixtures::rgb(400, 300, 0), 90);
    let input = fixtures::with_exif_orientation(&landscape, 6);
    let output = normalize_blocking(&input, FULL).unwrap();
    assert_eq!((output.width, output.height), (300, 400));
    assert_within(&output, FULL);
    let has = |needle: &[u8]| output.bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(&[0xFF, 0xD8]));
    assert!(!has(b"Exif"), "an EXIF segment survived re-encoding");
    assert!(!has(&[0xFF, 0xE1]), "an APP1 segment survived re-encoding");
    assert!(!has(b"ICC_PROFILE"));
    // Without the orientation the same source stays landscape.
    let plain = normalize_blocking(&landscape, FULL).unwrap();
    assert_eq!((plain.width, plain.height), (400, 300));
}

#[test]
fn real_transparency_stays_png_and_opaque_png_becomes_jpeg() {
    let transparent = fixtures::png_rgba8(&fixtures::rgba(600, 400, 0, true));
    let output = normalize_blocking(&transparent, FULL).unwrap();
    assert_eq!(output.mime, "image/png");
    assert!(decoded(&output).color().has_alpha());
    assert_within(&output, FULL);

    // An alpha channel that is fully opaque is not transparency.
    let opaque_rgba = fixtures::png_rgba8(&fixtures::rgba(600, 400, 0, false));
    let opaque_rgb = fixtures::png_rgb8(&fixtures::rgb(600, 400, 0));
    for input in [opaque_rgba, opaque_rgb] {
        let output = normalize_blocking(&input, FULL).unwrap();
        assert_eq!(output.mime, "image/jpeg");
        assert_within(&output, FULL);
    }
}

#[test]
fn transparency_too_large_for_png_is_flattened_onto_white() {
    let input = fixtures::png_rgba8(&fixtures::rgba(2048, 2048, 255, true));
    let output = normalize_blocking(&input, FULL).unwrap();
    assert_eq!(output.mime, "image/jpeg");
    assert_within(&output, FULL);
    // The fully transparent left edge became white, not black.
    let rgb = decoded(&output).to_rgb8();
    let corner = rgb.get_pixel(0, output.height / 2).0;
    assert!(corner.iter().all(|&channel| channel > 200), "{corner:?}");
}

#[test]
fn a_gif_yields_its_first_frame() {
    let input = fixtures::gif(64, 48, &[[220, 20, 20], [20, 20, 220]], false);
    let output = normalize_blocking(&input, FULL).unwrap();
    assert_eq!((output.width, output.height), (64, 48));
    let [red, _, blue] = decoded(&output).to_rgb8().get_pixel(32, 24).0;
    assert!(red > 180 && blue < 80, "not the first frame: {red} {blue}");
}

#[test]
fn a_pixel_bomb_is_refused_from_its_header_before_any_decode() {
    let input = fixtures::png_header_only(20_000, 20_000);
    let started = StdInstant::now();
    let refused = normalize_blocking(&input, FULL);
    assert_eq!(refused.unwrap_err(), ImageError::TooLargePixels);
    // Decoding 400 megapixels could not finish this fast, or at all.
    assert!(started.elapsed() < Duration::from_millis(100));
    // Under the side limit but over the pixel limit.
    let wide = fixtures::png_without_pixels(11_000, 3_000);
    assert_eq!(
        normalize_blocking(&wide, FULL).unwrap_err(),
        ImageError::TooLargePixels
    );
}

#[test]
fn the_header_admits_the_pixel_limit_and_refuses_one_row_more() {
    let rows = u32::try_from(MAX_PIXELS / 6000).unwrap();
    // Admitted from the header, so only the missing pixels fail the decode.
    let at = fixtures::png_without_pixels(6000, rows);
    assert_eq!(
        normalize_blocking(&at, FULL).unwrap_err(),
        ImageError::DecodeFailed
    );
    let over = fixtures::png_without_pixels(6000, rows + 1);
    assert_eq!(
        normalize_blocking(&over, FULL).unwrap_err(),
        ImageError::TooLargePixels
    );
}

#[test]
fn a_gif_frame_larger_than_its_screen_is_refused_before_it_is_allocated() {
    // The header declares 16x16, which passes every header check; the frame
    // alone would need a 529 MB buffer.
    let input = fixtures::gif_with_frame(16, 11_500);
    assert!(input.len() < 100 * 1024);
    assert_eq!(
        normalize_blocking(&input, FULL).unwrap_err(),
        ImageError::TooLargePixels
    );
    // The same truncated frame within its screen is decoded, and fails only
    // for the missing pixels.
    assert_eq!(
        normalize_blocking(&fixtures::gif_with_frame(16, 16), FULL).unwrap_err(),
        ImageError::DecodeFailed
    );
}

#[test]
fn a_png_with_icc_profiles_normalizes_exactly_as_without_them() {
    let plain = fixtures::png_rgb8(&fixtures::rgb(64, 48, 8));
    let expected = normalize_blocking(&plain, FULL).unwrap();
    // The profile is never applied, so removing it changes no output byte:
    // one that would inflate to 511 MiB, two in a row, and one after the
    // image data, which is left in place.
    let bomb = fixtures::with_icc_profile(&plain, 511 << 20);
    assert!(bomb.len() < 1024 * 1024);
    let twice = fixtures::with_icc_profile(&fixtures::with_icc_profile(&plain, 4096), 4096);
    let late = fixtures::with_icc_profile_after_pixels(&plain, 4096);
    for input in [bomb, twice, late] {
        let output = normalize_blocking(&input, FULL).unwrap();
        assert_eq!(output.bytes, expected.bytes);
        assert_eq!((output.width, output.height), (64, 48));
    }
}

#[test]
fn a_png_cut_inside_its_icc_profile_fails_to_decode() {
    let profiled = fixtures::with_icc_profile(&fixtures::png_rgb8(&fixtures::rgb(64, 48, 8)), 4096);
    let profile = profiled
        .windows(4)
        .position(|window| window == b"iCCP")
        .unwrap();
    // The profile chunk's length now runs past the input, and nothing
    // follows it.
    assert_eq!(
        normalize_blocking(&profiled[..profile + 16], FULL).unwrap_err(),
        ImageError::DecodeFailed
    );
}

#[test]
fn a_truncated_jpeg_fails_to_decode() {
    let input = fixtures::jpeg(&fixtures::rgb(64, 64, 8), 90);
    // Cut inside the quantization tables, before any frame header.
    assert_eq!(
        normalize_blocking(&input[..100], FULL).unwrap_err(),
        ImageError::DecodeFailed
    );
}

#[test]
fn formats_other_than_png_jpeg_gif_and_webp_are_unsupported() {
    let tiff = b"II*\0\x08\0\0\0\0\0\0\0\0\0\0\0".to_vec();
    let mut heic = 24_u32.to_be_bytes().to_vec();
    heic.extend_from_slice(b"ftypheic\0\0\0\0mif1heic");
    heic.extend_from_slice(&[0; 16]);
    for input in [tiff, heic, b"plain text, not an image".to_vec(), Vec::new()] {
        assert_eq!(
            normalize_blocking(&input, FULL).unwrap_err(),
            ImageError::UnsupportedFormat
        );
    }
}

/// Whether `image` itself decodes `input`: a WebP it decodes and
/// normalization refuses is kept from its decoder by the refusal alone.
fn image_decodes(input: &[u8]) -> bool {
    image::load_from_memory(input).is_ok()
}

fn assert_unsupported(input: &[u8]) {
    assert_eq!(
        normalize_blocking(input, FULL).unwrap_err(),
        ImageError::UnsupportedFormat
    );
}

/// [`webp::LOSSY`]'s image in a lossless WebP of its own.
fn lossless_webp() -> Vec<u8> {
    fixtures::webp_lossless(&fixtures::rgba(webp::WIDTH, webp::HEIGHT, 0, false))
}

#[test]
fn a_lossy_webp_normalizes() {
    let output = normalize_blocking(webp::LOSSY, FULL).unwrap();
    assert_eq!((output.width, output.height), (webp::WIDTH, webp::HEIGHT));
    assert_eq!(output.mime, "image/jpeg");
    assert_within(&output, FULL);
    // The gradient survived: red rises from left to right.
    let rgb = decoded(&output).to_rgb8();
    let [left, ..] = rgb.get_pixel(2, 24).0;
    let [right, ..] = rgb.get_pixel(61, 24).0;
    assert!(left < 40 && right > 200, "{left} {right}");
}

#[test]
fn a_lossless_webp_is_refused_before_it_is_decoded() {
    let input = lossless_webp();
    assert!(image_decodes(&input));
    assert_unsupported(&input);
}

#[test]
fn an_animation_with_any_lossless_frame_is_refused() {
    let lossless = webp::lossless_frame(&lossless_webp(), webp::WIDTH, webp::HEIGHT);
    // `image` decodes only the first frame, but every frame is checked.
    for frames in [vec![lossless.clone()], vec![webp::lossy_frame(), lossless]] {
        let input = webp::animation(0, webp::WIDTH, webp::HEIGHT, &frames);
        assert!(image_decodes(&input));
        assert_unsupported(&input);
    }
    // Lossy frames alone are decoded, and the first is shown.
    let lossy = webp::animation(
        0,
        webp::WIDTH,
        webp::HEIGHT,
        &[webp::lossy_frame(), webp::lossy_frame()],
    );
    let output = normalize_blocking(&lossy, FULL).unwrap();
    assert_eq!((output.width, output.height), (webp::WIDTH, webp::HEIGHT));
}

#[test]
fn lossy_webp_with_lossless_alpha_is_refused() {
    // libwebp compresses a lossy image's alpha losslessly by default, and the
    // lossless decoder reads that alpha, in a still image and in a frame.
    let animated = webp::animation(
        webp::ALPHA,
        webp::WIDTH,
        webp::HEIGHT,
        &[webp::lossy_alpha_frame()],
    );
    for input in [webp::LOSSY_ALPHA, &animated] {
        assert!(image_decodes(input));
        assert_unsupported(input);
    }
}

#[test]
fn lossy_webp_with_raw_alpha_keeps_its_transparency() {
    let (width, height) = (webp::WIDTH, webp::HEIGHT);
    // Transparent on the left half, opaque on the right.
    let alpha: Vec<u8> = (0..width * height)
        .map(|index| {
            if index % width < width / 2 {
                0
            } else {
                u8::MAX
            }
        })
        .collect();
    let lossy = webp::find_chunk(webp::LOSSY, b"VP8 ");
    let still = webp::riff(&[
        webp::extended(webp::ALPHA, width, height),
        webp::raw_alpha(&alpha),
        lossy.clone(),
    ]);
    let animated = webp::animation(
        webp::ALPHA,
        width,
        height,
        &[webp::frame(
            width,
            height,
            &[webp::raw_alpha(&alpha), lossy],
        )],
    );
    for input in [still, animated] {
        let output = normalize_blocking(&input, FULL).unwrap();
        assert_eq!(output.mime, "image/png");
        let rgba = decoded(&output).to_rgba8();
        assert_eq!(rgba.get_pixel(2, 24).0[3], 0);
        assert_eq!(rgba.get_pixel(61, 24).0[3], u8::MAX);
    }
}

#[test]
fn a_lossless_chunk_is_found_wherever_the_decoder_reads_it() {
    let lossless = webp::find_chunk(&lossless_webp(), b"VP8L");
    let header = webp::extended(0, webp::WIDTH, webp::HEIGHT);
    // After a chunk of odd length, past its padding byte.
    let padded = webp::riff(&[
        header.clone(),
        webp::chunk(b"XTRA", &[1, 2, 3]),
        lossless.clone(),
    ]);
    // Past the end the RIFF size declares, which the decoder reads beyond.
    let mut overread = webp::riff(&[header]);
    overread.extend_from_slice(&lossless);
    // Lossless alpha in a frame of a still image, whose first chunks the
    // decoder takes as the image's own: only the alpha's header lies inside
    // the frame, and the decoder reads its data from past the frame's end.
    let alpha = webp::find_chunk(webp::LOSSY_ALPHA, b"ALPH");
    let (alpha_header, alpha_data) = alpha.split_at(8);
    let outside = webp::riff(&[
        webp::extended(webp::ALPHA, webp::WIDTH, webp::HEIGHT),
        webp::find_chunk(webp::LOSSY, b"VP8 "),
        webp::frame(webp::WIDTH, webp::HEIGHT, &[alpha_header.to_vec()]),
        alpha_data.to_vec(),
    ]);
    for input in [padded, overread, outside] {
        assert!(image_decodes(&input));
        assert_unsupported(&input);
    }
}

#[test]
fn truncated_or_garbage_webp_never_panics() {
    let animated = webp::animation(
        webp::ALPHA,
        webp::WIDTH,
        webp::HEIGHT,
        &[webp::lossy_frame(), webp::lossy_alpha_frame()],
    );
    // Every cut of every sample returns, with an error or, when only the end
    // of a lossy bitstream is missing, which its decoder tolerates, with the
    // whole image.
    for sample in [webp::LOSSY, webp::LOSSY_ALPHA, &animated, &lossless_webp()] {
        for end in 0..sample.len() {
            if let Ok(output) = normalize_blocking(&sample[..end], FULL) {
                let size = (output.width, output.height);
                assert_eq!(size, (webp::WIDTH, webp::HEIGHT), "cut at {end}");
            }
        }
    }
    // Lossless alpha is refused from its first byte on, before its chunk is
    // complete. Cut before that byte, the image fails to decode.
    let alpha = webp::LOSSY_ALPHA
        .windows(4)
        .position(|window| window == b"ALPH")
        .unwrap()
        + 8;
    assert_unsupported(&webp::LOSSY_ALPHA[..=alpha]);
    assert_eq!(
        normalize_blocking(&webp::LOSSY_ALPHA[..alpha], FULL).unwrap_err(),
        ImageError::DecodeFailed
    );
    // Chunk lengths of 4 GiB, odd, with almost nothing after them: the one
    // naming the lossless decoder is refused, the others fail to decode.
    let huge = |before: &[Vec<u8>], kind: &[u8; 4], data: &[u8]| {
        let mut out = b"RIFF\xFF\xFF\xFF\xFFWEBP".to_vec();
        out.extend_from_slice(&before.concat());
        out.extend_from_slice(kind);
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(data);
        out
    };
    assert_unsupported(&huge(&[], b"VP8L", &[0x2F, 0, 0, 0]));
    let animation = webp::extended(webp::ANIMATION, webp::WIDTH, webp::HEIGHT);
    for input in [
        huge(&[], b"VP8X", &[0; 10]),
        huge(&[], b"ALPH", &[]),
        huge(&[animation], b"ANMF", &[0; 20]),
    ] {
        assert_eq!(
            normalize_blocking(&input, FULL).unwrap_err(),
            ImageError::DecodeFailed
        );
    }
    // Noise after a valid signature.
    let mut random = fixtures::Noise::new(11);
    let mut noise = b"RIFF\x00\x10\x00\x00WEBP".to_vec();
    noise.extend((0..4096).map(|_| random.byte()));
    assert!(normalize_blocking(&noise, FULL).is_err());
}

#[test]
fn previews_fit_their_budget() {
    let inputs = [
        fixtures::jpeg(&fixtures::rgb(3000, 2000, 40), 95),
        fixtures::png_rgba8(&fixtures::rgba(1600, 1600, 64, true)),
        // A lossy frame on a canvas larger than the preview.
        webp::animation(0, 1200, 900, &[webp::lossy_frame()]),
    ];
    for input in inputs {
        let output = normalize_blocking(&input, PREVIEW).unwrap();
        assert_within(&output, PREVIEW);
    }
}

#[test]
fn an_incompressible_image_steps_down_until_it_fits() {
    let input = fixtures::png_rgb8(&fixtures::rgb(2048, 2048, 255));
    let output = normalize_blocking(&input, FULL).unwrap();
    assert!(output.width <= 1536, "did not step down: {output:?}");
    assert_eq!(output.width, output.height);
    assert_within(&output, FULL);
}

#[tokio::test]
async fn normalization_runs_on_the_blocking_pool() {
    let decode = Arc::new(Semaphore::new(1));
    let input = Bytes::from(fixtures::jpeg(&fixtures::rgb(800, 600, 8), 90));
    let deadline = Instant::now() + Duration::from_secs(30);
    let output = normalize(input, FULL, &decode, deadline).await.unwrap();
    assert_eq!((output.width, output.height), (800, 600));
    assert_eq!(output.at_ms, None);
    // The permit was released when the blocking task ended.
    assert_eq!(decode.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_held_decode_permit_times_out_at_the_deadline() {
    let decode = Arc::new(Semaphore::new(1));
    let _held = decode.clone().acquire_owned().await.unwrap();
    let input = Bytes::from(fixtures::jpeg(&fixtures::rgb(8, 8, 0), 90));
    let deadline = Instant::now() + Duration::from_secs(5);
    let refused = normalize(input, FULL, &decode, deadline).await;
    assert_eq!(refused.unwrap_err(), ImageError::TimedOut);
    assert!(Instant::now() >= deadline);
}

#[test]
fn errors_have_stable_codes() {
    let codes: Vec<_> = [
        ImageError::UnsupportedFormat,
        ImageError::TooLargePixels,
        ImageError::DecodeFailed,
        ImageError::TooLarge,
        ImageError::TimedOut,
    ]
    .iter()
    .map(|error| error.to_string())
    .collect();
    assert_eq!(
        codes,
        [
            "unsupported_format",
            "too_large_pixels",
            "decode_failed",
            "too_large",
            "timed_out"
        ]
    );
}
