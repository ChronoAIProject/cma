//! The console's channel memory against the fake CMA: the owner index built
//! from paced record-head listings, and its gauges.
use std::{sync::Arc, time::Duration};

use cmaeg::{
    console::{
        cma_budget::Pacing,
        memory::{ConsoleMemory, Schedule},
    },
    records::RecordClient,
    ConsoleMemoryConfig,
};

use crate::fake_cma::FakeCma;

fn schedule() -> Schedule {
    Schedule {
        settle: Duration::ZERO,
        resync: Duration::from_secs(3_600),
        pacing: Pacing {
            base: Duration::from_millis(10),
        },
    }
}

/// `count` records in the fake CMA, alternating between two owners.
async fn seeded(count: usize) -> FakeCma {
    let cma = FakeCma::start(false).await;
    for index in 0..count {
        let owner = if index % 2 == 0 { "alice" } else { "bob" };
        cma.seed(
            &format!("00000000-0000-4000-8000-{index:012}"),
            owner,
            b"sealed".to_vec(),
        );
    }
    cma
}

#[tokio::test]
async fn the_index_lists_every_seeded_record_and_exports_its_gauges() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let cma = seeded(1_000).await;
    let records = Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap());
    let memory = ConsoleMemory::new(&ConsoleMemoryConfig::default(), records, schedule());
    memory.list_all().await;
    let status = memory.index_status();
    assert_eq!(status.records, 1_000);
    assert!(!status.partial);
    assert_eq!(status.loading, None);
    // Ten full pages and the short one that ends the listing.
    assert_eq!(cma.lists.load(std::sync::atomic::Ordering::SeqCst), 11);
    let text = metrics.render();
    assert!(text.contains("cmaeg_console_index_records 1000"), "{text}");
    assert!(text.contains("cmaeg_console_index_partial 0"), "{text}");
    cma.close().await;
}

#[tokio::test]
async fn a_bounded_index_reports_itself_partial() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let cma = seeded(1_100).await;
    let records = Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap());
    let config = ConsoleMemoryConfig {
        index_max_records: 1_000,
        ..ConsoleMemoryConfig::default()
    };
    let memory = ConsoleMemory::new(&config, records, schedule());
    memory.list_all().await;
    assert_eq!(memory.index_status().records, 1_000);
    assert!(memory.index_status().partial);
    let text = metrics.render();
    assert!(text.contains("cmaeg_console_index_partial 1"), "{text}");
    cma.close().await;
}
