//! Image normalization: budgets, orientation, metadata stripping, format
//! handling and refusals.
use super::image_fixtures as fixtures;
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

#[test]
fn previews_fit_their_budget() {
    let inputs = [
        fixtures::jpeg(&fixtures::rgb(3000, 2000, 40), 95),
        fixtures::png_rgba8(&fixtures::rgba(1600, 1600, 64, true)),
        fixtures::webp_lossless(&fixtures::rgba(1200, 900, 32, false)),
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
