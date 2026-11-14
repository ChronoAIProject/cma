//! Run in each service's binary harness so allocations use its selected allocator.

#[repr(align(64))]
#[derive(Clone)]
struct Aligned([u8; 64]);

#[test]
fn aligned_buffers_survive_cross_thread_growth_and_drop() {
    std::thread::scope(|scope| {
        let mut senders = Vec::new();
        let mut workers = Vec::new();
        for _ in 0..8 {
            let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<Aligned>>(1);
            senders.push(sender);
            workers.push(scope.spawn(move || {
                let mut batches = 0;
                while let Ok(mut buffer) = receiver.recv() {
                    let marker = buffer[0].0[0];
                    assert_eq!(buffer.as_ptr() as usize % 64, 0);
                    buffer.reserve_exact(513);
                    buffer.resize(513, Aligned([marker; 64]));
                    assert_eq!(buffer.as_ptr() as usize % 64, 0);
                    assert!(buffer.iter().all(|item| item.0 == [marker; 64]));
                    batches += 1;
                    // Free on a different worker from the original allocation.
                    drop(buffer);
                }
                batches
            }));
        }
        for round in 0..32u8 {
            for sender in &senders {
                sender
                    .send(vec![Aligned([round; 64]); 1 + usize::from(round)])
                    .unwrap();
            }
        }
        drop(senders);
        for worker in workers {
            assert_eq!(worker.join().unwrap(), 32);
        }
    });
}

#[test]
fn zeroed_buffer_preserves_contents_when_capacity_request_overflows() {
    let mut buffer = vec![0u64; 4096];
    assert!(buffer.iter().all(|value| *value == 0));
    buffer[0] = 17;
    buffer[4095] = 29;
    let capacity = buffer.capacity();
    assert!(buffer.try_reserve(usize::MAX).is_err());
    assert_eq!(buffer.capacity(), capacity);
    assert_eq!(buffer.len(), 4096);
    assert_eq!((buffer[0], buffer[4095]), (17, 29));
    assert!(buffer[1..4095].iter().all(|value| *value == 0));
}
