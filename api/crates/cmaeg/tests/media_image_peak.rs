//! The decode memory and wall-time bounds of image normalization, measured.
//!
//! This is its own test binary so its counting global allocator affects no
//! other test. The allocator counts every thread of the process, and
//! `cargo test` runs separate tests of one binary on parallel threads (nextest
//! isolates processes, plain `cargo test` does not), so every fixture runs
//! sequentially inside the one test below.
#[path = "media/image_fixtures.rs"]
pub mod image_fixtures;

use cmaeg::media::image::{
    normalize_blocking, ImageError, NormalizedImage, DECODE_PEAK_BUDGET, DECODE_WALL_BUDGET, FULL,
    MAX_PIXELS,
};
use image_fixtures as fixtures;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

impl Counting {
    fn grew(size: usize) {
        let current = CURRENT.fetch_add(size, Ordering::SeqCst) + size;
        PEAK.fetch_max(current, Ordering::SeqCst);
    }

    fn shrank(size: usize) {
        CURRENT.fetch_sub(size, Ordering::SeqCst);
    }
}

// SAFETY: every call is forwarded unchanged to `System`; the counters only
// observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is forwarded unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            Self::grew(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is forwarded unchanged.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            Self::grew(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract is forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) };
        Self::shrank(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's contract is forwarded unchanged.
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            if new_size >= layout.size() {
                Self::grew(new_size - layout.size());
            } else {
                Self::shrank(layout.size() - new_size);
            }
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MIB: f64 = 1024.0 * 1024.0;
/// The photo fixtures are 3:2 or wider at the pixel limit, so they test the
/// bound rather than a size under it.
const WIDTH: u32 = 6000;
const HEIGHT: u32 = (MAX_PIXELS / WIDTH as u64) as u32;
/// The largest input `CMAEG_MEDIA_MAX_BYTES` admits. A JPEG decoder holds a
/// copy of its whole input, so the worst JPEG fixture is near this size.
const MAX_INPUT_BYTES: usize = 25 * 1024 * 1024;

/// Normalize `input` to FULL and assert the peak above the baseline and the
/// wall time, whatever the outcome: a refusal is bounded like a success.
fn bounded(name: &str, input: &[u8]) -> Result<NormalizedImage, ImageError> {
    let baseline = CURRENT.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    let started = Instant::now();
    let result = normalize_blocking(input, FULL);
    let elapsed = started.elapsed();
    let peak = PEAK.load(Ordering::SeqCst) - baseline;
    let outcome = match &result {
        Ok(output) => format!(
            "output {}x{} {} {:.1} KiB",
            output.width,
            output.height,
            output.mime,
            output.bytes.len() as f64 / 1024.0
        ),
        Err(error) => format!("refused {error}"),
    };
    println!(
        "{name}: input {:.1} MiB, peak {:.1} MiB, {elapsed:?}, {outcome}",
        input.len() as f64 / MIB,
        peak as f64 / MIB,
    );
    assert!(
        peak <= DECODE_PEAK_BUDGET,
        "{name}: peak {peak} exceeds {DECODE_PEAK_BUDGET}"
    );
    assert!(
        elapsed <= DECODE_WALL_BUDGET,
        "{name}: {elapsed:?} exceeds {DECODE_WALL_BUDGET:?}"
    );
    result
}

/// [`bounded`] for a fixture that must normalize within the output budget.
fn measure(name: &str, input: &[u8]) {
    let output = bounded(name, input)
        .unwrap_or_else(|error| panic!("{name}: normalization failed: {error}"));
    assert!(
        output.bytes.len() <= FULL.max_bytes,
        "{name}: output too large"
    );
}

/// [`measure`] for a fixture whose cost grows with its input size, which
/// must therefore be near the largest input admitted.
fn measure_near_input_limit(name: &str, input: &[u8]) {
    assert!(
        (MAX_INPUT_BYTES * 9 / 10..=MAX_INPUT_BYTES).contains(&input.len()),
        "{name}: {} bytes is not near the input limit",
        input.len()
    );
    measure(name, input);
}

/// [`bounded`] for a fixture that must be refused with `expected`.
fn refused(name: &str, input: &[u8], expected: ImageError) {
    match bounded(name, input) {
        Err(error) => assert_eq!(error, expected, "{name}"),
        Ok(output) => panic!("{name}: normalized to {output:?}"),
    }
}

#[test]
fn image_decode_bounds() {
    let photo = fixtures::rgb(WIDTH, HEIGHT, 24);
    let baseline = fixtures::with_exif_orientation(&fixtures::jpeg(&photo, 90), 6);
    measure(&format!("baseline jpeg {WIDTH}x{HEIGHT} exif 6"), &baseline);
    drop(baseline);
    let progressive = fixtures::progressive_jpeg(&photo, 90);
    drop(photo);
    measure(
        &format!("progressive 4:4:4 jpeg {WIDTH}x{HEIGHT}"),
        &progressive,
    );
    drop(progressive);
    // Four planes of coefficients beside the RGB output, and a copy of an
    // input near the largest admitted: the slowest JPEG decode.
    let cmyk = fixtures::progressive_cmyk_jpeg(WIDTH, HEIGHT, 39, 90);
    measure_near_input_limit(
        &format!("progressive 4:4:4 cmyk jpeg {WIDTH}x{HEIGHT}"),
        &cmyk,
    );
    drop(cmyk);
    // The decoder also keeps a copy of every metadata segment, so the same
    // image at its smallest, padded with ICC segments to the input limit,
    // makes it hold the most beside its pixels.
    let flat = fixtures::progressive_cmyk_jpeg(WIDTH, HEIGHT, 0, 90);
    let padded = fixtures::with_icc_segments(&flat, MAX_INPUT_BYTES);
    drop(flat);
    measure_near_input_limit(
        &format!("progressive 4:4:4 cmyk jpeg {WIDTH}x{HEIGHT} padded with icc segments"),
        &padded,
    );
    drop(padded);

    let rgba = fixtures::rgba(WIDTH, HEIGHT, 24, false);
    let png = fixtures::png_rgba8(&rgba);
    measure(&format!("rgba8 png {WIDTH}x{HEIGHT}"), &png);
    // Removing an ICC profile copies the input, so the copy is measured
    // beside the largest PNG pixels, with a profile that would have inflated
    // to 300 MiB beside them.
    let profiled = fixtures::with_icc_profile(&png, 300 << 20);
    drop(png);
    measure(
        &format!("rgba8 png {WIDTH}x{HEIGHT} with a 300 MiB icc profile"),
        &profiled,
    );
    drop(profiled);
    let webp = fixtures::webp_lossless(&rgba);
    drop(rgba);
    measure(&format!("lossless webp {WIDTH}x{HEIGHT}"), &webp);
    drop(webp);

    // Bounded by the allocation limit rather than the pixel limit.
    let deep = fixtures::png_rgba16(4000, 4000);
    measure("rgba16 png 4000x4000", &deep);
    drop(deep);

    // A decoder inflates a PNG's ICC profile before any pixel, bounded only
    // by its 512 MiB allocation limit, and drops a larger one instead of
    // refusing the image, so a profile just under that limit costs the most.
    // Before profiles were removed this 512x512 PNG peaked at 529.5 MiB.
    let small = fixtures::png_rgb8(&fixtures::rgb(512, 512, 8));
    let bomb = fixtures::with_icc_profile(&small, 511 << 20);
    measure("rgb8 png 512x512 with a 511 MiB icc profile", &bomb);
    drop(bomb);
    let late = fixtures::with_icc_profile_after_pixels(&small, 511 << 20);
    measure(
        "rgb8 png 512x512 with a 511 MiB icc profile after the image data",
        &late,
    );
    drop(late);

    let side = (MAX_PIXELS as f64).sqrt() as u32;
    let gif = fixtures::gif(side, side, &[[200, 40, 40], [40, 40, 200]], true);
    measure(&format!("two-frame gif {side}x{side}"), &gif);
    drop(gif);

    // Its header passes every check; decoded without the limits, the frame
    // alone took a 529 MB buffer.
    let frame_bomb = fixtures::gif_with_frame(16, 11_500);
    refused(
        "gif 16x16 screen with an 11500x11500 frame",
        &frame_bomb,
        ImageError::TooLargePixels,
    );
}
