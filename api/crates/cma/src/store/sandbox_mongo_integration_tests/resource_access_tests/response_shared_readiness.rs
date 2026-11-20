use super::events;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task::JoinHandle;

pub(super) async fn wait_for_replay(buffers: &[Arc<Mutex<Vec<u8>>>], consumers: &[JoinHandle<()>]) {
    // Headers precede replay. Include the complete cold opening in the meter
    // before starting the unchanged-observation window for every consumer.
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut pending: Vec<_> = buffers.iter().collect();
        while !pending.is_empty() {
            assert!(consumers.iter().all(|consumer| !consumer.is_finished()));
            pending.retain(|buffer| {
                let buffer = buffer.lock().unwrap();
                let Some(end) = buffer.iter().rposition(|byte| *byte == b'\n') else {
                    return true;
                };
                !events(&buffer[..=end]).last().is_some_and(|event| {
                    event["type"] == "response.output_item.done" && event["sequence_number"] == 7
                })
            });
            if !pending.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    })
    .await
    .expect("every observer must finish the retained eight-event replay");
}
