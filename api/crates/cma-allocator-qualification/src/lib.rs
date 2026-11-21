//! Qualify the linked allocator, including frees on persistent foreign threads.
use std::{
    alloc::{alloc, dealloc, realloc, Layout},
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const WORKERS: usize = 18;
const BLOCKS: usize = 128;
const WARMUP: usize = 256;
const ROUNDS: usize = 4096;
const RETAINED_LIMIT_KIB: usize = 24 * 1024;
const DEADLINE: Duration = Duration::from_secs(120);
static LIVE: AtomicUsize = AtomicUsize::new(0);

struct Block {
    pointer: usize,
    layout: Layout,
}

impl Block {
    fn new(size: usize, alignment: usize, grow: bool) -> io::Result<Self> {
        let layout = Layout::from_size_align(size, alignment).map_err(io::Error::other)?;
        let grown_layout =
            Layout::from_size_align(size + 7, alignment).map_err(io::Error::other)?;
        // SAFETY: the valid layout is retained with the allocation until Drop.
        let pointer = unsafe { alloc(layout) };
        if pointer.is_null() {
            return Err(io::Error::other("allocator refused bounded test block"));
        }
        let mut block = Self {
            pointer: pointer as usize,
            layout,
        };
        LIVE.fetch_add(1, Ordering::Relaxed);
        // Commit every byte: virtual address reservations cannot pass an RSS test.
        // SAFETY: this pointer owns layout.size() valid writable bytes.
        unsafe { pointer.write_bytes(17, size) };
        if grow {
            // SAFETY: pointer/layout still identify this live allocation; the
            // new nonzero size preserves alignment. Failure keeps the old block.
            let moved = unsafe { realloc(pointer, layout, size + 7) };
            if moved.is_null() {
                return Err(io::Error::other("allocator refused bounded test resize"));
            }
            block.pointer = moved as usize;
            block.layout = grown_layout;
        }
        Ok(block)
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        // SAFETY: Block has sole ownership, is never cloned, and records the
        // current layout after realloc. Moving it across threads transfers ownership.
        unsafe { dealloc(self.pointer as *mut u8, self.layout) };
        LIVE.fetch_sub(1, Ordering::Release);
    }
}

fn rss_kib() -> io::Result<usize> {
    // Missing procfs or an unsupported platform is a failed qualification.
    // Never report a skipped platform as a successful memory bound.
    let status = std::fs::read_to_string("/proc/self/status")?;
    parse_rss_kib(&status)
}

fn parse_rss_kib(status: &str) -> io::Result<usize> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.trim().strip_suffix("kB"))
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| io::Error::other("VmRSS in KiB is unavailable"))
}

fn check_live_blocks(live: usize) -> io::Result<()> {
    if live != 0 {
        return Err(io::Error::other(
            "allocator test retained live payload blocks",
        ));
    }
    Ok(())
}

fn check_deadline(elapsed: Duration) -> io::Result<()> {
    if elapsed >= DEADLINE {
        return Err(io::Error::other(
            "allocator qualification deadline exceeded",
        ));
    }
    Ok(())
}

fn check_retention(baseline: usize, rss: usize) -> io::Result<()> {
    let retained = rss.saturating_sub(baseline);
    if retained > RETAINED_LIMIT_KIB {
        return Err(io::Error::other(format!(
            "freed allocations retained {retained} KiB above warm baseline; limit {RETAINED_LIMIT_KIB} KiB"
        )));
    }
    Ok(())
}

fn sample(boundary: &str, started: Instant) -> io::Result<usize> {
    let live = LIVE.load(Ordering::Acquire);
    check_live_blocks(live)?;
    let rss = rss_kib()?;
    println!(
        "allocator-self-test boundary={boundary} live_blocks={live} rss_kib={rss} elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(rss)
}

fn handoff_round(
    round: usize,
    mut send: impl FnMut(usize, Vec<Block>) -> io::Result<()>,
) -> io::Result<()> {
    // Warm the same peak live working set regardless of how quickly workers free.
    // Handing off during allocation lets scheduling inflate RSS only after warmup.
    let mut batches = Vec::with_capacity(WORKERS);
    for worker in 0..WORKERS {
        let mut blocks = Vec::with_capacity(BLOCKS);
        for item in 0..BLOCKS {
            let size = [31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16351, 32735]
                [(item + worker + round) % 11];
            blocks.push(Block::new(
                size,
                if item % 7 == 0 { 64 } else { 8 },
                item % 3 == 0,
            )?);
        }
        batches.push(blocks);
    }
    for (worker, blocks) in batches.into_iter().enumerate() {
        send(worker, blocks)?;
    }
    Ok(())
}

fn exercise(
    senders: &[mpsc::SyncSender<Vec<Block>>],
    done: &mpsc::Receiver<()>,
    started: Instant,
) -> io::Result<usize> {
    let mut baseline = 0;
    let mut retained = 0;
    for round in 0..ROUNDS {
        check_deadline(started.elapsed())?;
        handoff_round(round, |worker, blocks| {
            senders[worker]
                .send(blocks)
                .map_err(|_| io::Error::other("allocator worker exited before handoff"))
        })?;
        // An acknowledgement follows Drop of the complete batch, including its
        // vector. No payload allocation remains live at any measured boundary.
        for _ in 0..WORKERS {
            done.recv_timeout(Duration::from_secs(10))
                .map_err(|_| io::Error::other("allocator worker did not finish"))?;
        }
        let completed = round + 1;
        if completed == WARMUP || completed % 512 == 0 {
            let rss = sample(&completed.to_string(), started)?;
            if completed == WARMUP {
                baseline = rss;
            } else {
                retained = retained.max(rss.saturating_sub(baseline));
            }
        }
    }
    check_retention(0, retained)?;
    Ok(baseline)
}

pub fn run() -> io::Result<()> {
    let started = Instant::now();
    rss_kib()?;
    let status = std::fs::read_to_string("/proc/self/status")?;
    let affinity = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .ok_or_else(|| io::Error::other("CPU affinity is unavailable"))?;
    println!("allocator-self-test cpu_affinity={}", affinity.trim());
    println!(
        "allocator-self-test workers={WORKERS} blocks_per_worker={BLOCKS} rounds={ROUNDS} warmup={WARMUP} retained_limit_kib={RETAINED_LIMIT_KIB}"
    );
    let (acknowledge, done) = mpsc::sync_channel(WORKERS);
    let mut senders = Vec::new();
    let mut workers = Vec::new();
    let result = (|| {
        for _ in 0..WORKERS {
            let (sender, receiver) = mpsc::sync_channel::<Vec<Block>>(1);
            let acknowledge = acknowledge.clone();
            workers.push(thread::Builder::new().spawn(move || {
                while let Ok(blocks) = receiver.recv() {
                    drop(blocks);
                    if acknowledge.send(()).is_err() {
                        break;
                    }
                }
            })?);
            senders.push(sender);
        }
        exercise(&senders, &done, started)
    })();
    // Close both directions before joining, including partial startup/error
    // paths, so a blocked acknowledgement cannot strand a persistent worker.
    drop(senders);
    drop(done);
    let mut worker_failed = false;
    for worker in workers {
        worker_failed |= worker.join().is_err();
    }
    let final_sample = sample("quiesced", started);
    let baseline = result?;
    if worker_failed {
        return Err(io::Error::other("allocator qualification worker panicked"));
    }
    // Worker teardown belongs to qualification too. Neither late completion nor
    // retention seen only after the final round may turn into a successful gate.
    check_retention(baseline, final_sample?)?;
    check_deadline(started.elapsed())?;
    println!("allocator-self-test outcome=ok");
    Ok(())
}

#[cfg(test)]
#[path = "allocator_self_test/tests.rs"]
mod tests;
