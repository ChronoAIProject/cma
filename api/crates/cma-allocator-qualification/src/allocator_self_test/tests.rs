use super::*;

#[test]
fn procfs_rss_requires_a_measurement_in_kib() {
    assert_eq!(
        parse_rss_kib("Name:\tcmaeg\nVmRSS:\t 4096 kB\n").unwrap(),
        4096
    );
    for status in [
        "",
        "VmSize: 4096 kB\n",
        "VmRSS: unavailable kB\n",
        "VmRSS: -1 kB\n",
        "VmRSS: 4096 MB\n",
        "VmRSS: 184467440737095516160 kB\n",
    ] {
        assert!(parse_rss_kib(status).is_err(), "{status:?}");
    }
}

#[test]
fn every_measured_boundary_requires_zero_live_payloads() {
    assert!(check_live_blocks(0).is_ok());
    assert!(check_live_blocks(1).is_err());
    assert!(check_live_blocks(usize::MAX).is_err());
}

#[test]
fn retention_accepts_the_limit_but_rejects_one_kib_more() {
    let baseline = 4096;
    assert!(check_retention(baseline, baseline - 1).is_ok());
    assert!(check_retention(baseline, baseline + RETAINED_LIMIT_KIB).is_ok());
    assert!(check_retention(baseline, baseline + RETAINED_LIMIT_KIB + 1).is_err());
}

#[test]
fn the_recorded_bad_allocator_cannot_pass_the_bound() {
    // Equal-work incident controls, expressed as growth above their warm RSS.
    assert!(check_retention(0, 4096).is_ok());
    assert!(check_retention(0, 35_892).is_err());
}

#[test]
fn completion_at_or_after_the_deadline_is_a_failure() {
    assert!(check_deadline(DEADLINE - Duration::from_nanos(1)).is_ok());
    assert!(check_deadline(DEADLINE).is_err());
    assert!(check_deadline(DEADLINE + Duration::from_secs(1)).is_err());
}

#[test]
fn payload_ownership_covers_alignment_realloc_foreign_free_and_failed_handoff() {
    assert_eq!(LIVE.load(Ordering::Acquire), 0);
    assert!(Block::new(31, 3, false).is_err());
    let blocks = [false, true]
        .into_iter()
        .map(|grow| Block::new(31, 64, grow).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(LIVE.load(Ordering::Acquire), 2);
    for block in &blocks {
        assert_eq!(block.pointer % 64, 0);
        // SAFETY: each block owns at least the 31 initialized bytes from new.
        let bytes = unsafe { std::slice::from_raw_parts(block.pointer as *const u8, 31) };
        assert_eq!(bytes, &[17; 31]);
    }
    thread::spawn(move || drop(blocks)).join().unwrap();
    assert_eq!(LIVE.load(Ordering::Acquire), 0);

    let (sender, receiver) = mpsc::sync_channel(1);
    drop(receiver);
    let refused = sender.send(vec![Block::new(127, 8, true).unwrap()]);
    assert!(refused.is_err());
    drop(refused);
    assert_eq!(LIVE.load(Ordering::Acquire), 0);
}
