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

/// [`bounded`] for a fixture that must be refused with `expected`.
fn refused(name: &str, input: &[u8], expected: ImageError) {
    match bounded(name, input) {
        Err(error) => assert_eq!(error, expected, "{name}"),
        Ok(output) => panic!("{name}: normalized to {output:?}"),
    }
}

#[test]
fn image_decode_bounds() {
    let photo = fixtures::rgb(6000, 4000, 24);
    let baseline = fixtures::with_exif_orientation(&fixtures::jpeg(&photo, 90), 6);
    measure("baseline jpeg 6000x4000 exif 6", &baseline);
    drop(baseline);
    let progressive = fixtures::progressive_jpeg(&photo, 90);
    drop(photo);
    measure("progressive 4:4:4 jpeg 6000x4000", &progressive);
    drop(progressive);

    let rgba = fixtures::rgba(6000, 4000, 24, false);
    let png = fixtures::png_rgba8(&rgba);
    measure("rgba8 png 6000x4000", &png);
    drop(png);
    let webp = fixtures::webp_lossless(&rgba);
    drop(rgba);
    measure("lossless webp 6000x4000", &webp);
    drop(webp);

    let deep = fixtures::png_rgba16(4000, 4000);
    measure("rgba16 png 4000x4000", &deep);
    drop(deep);

    let gif = fixtures::gif(4800, 4800, &[[200, 40, 40], [40, 40, 200]], true);
    measure("two-frame gif 4800x4800", &gif);
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
