use super::*;
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::Mutex;

type Commands = Arc<Mutex<Vec<String>>>;

async fn observed() -> (ReportingStore, Commands) {
    let fixture = fixture().await;
    let commands = Commands::default();
    let captured = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some(ROWS)
                || (event.command_name == "getMore"
                    && event.command.get_str("collection").ok() == Some(ROWS))
            {
                captured.lock().unwrap().push(event.command_name);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let mut store = (*fixture).clone();
    store.persistence = store.persistence.with_client(client);
    (store, commands)
}

fn take_counts(commands: &Commands) -> (usize, usize) {
    let commands = std::mem::take(&mut *commands.lock().unwrap());
    (
        commands.iter().filter(|name| *name == "find").count(),
        commands.iter().filter(|name| *name == "getMore").count(),
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_pages_keep_lookahead_and_avoid_default_batch_get_more() {
    let (store, commands) = observed().await;
    let at = now();
    let mut report = store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "page-batch")
        .await
        .unwrap()
        .unwrap();
    let rows: Vec<_> = (0..225).map(|n| fixture_row(&state(n, at - 60))).collect();
    store
        .write_report_chunk(&lease, &rows, vec![])
        .await
        .unwrap();
    report.summary = Some(Summary {
        sandbox_count: rows.len() as u64,
        ..Default::default()
    });
    store
        .finish_report(&lease, report.clone(), true)
        .await
        .unwrap();

    // Small fixture rows fit in one server batch. The old query needs a
    // getMore after Mongo's default first 101 documents, including lookahead.
    take_counts(&commands);
    let baseline: Vec<Document> = store
        .collection(ROWS)
        .find(doc! {"report_id":&report.report_id})
        .sort(doc! {"nulls.name":1,"sorts.name":1,"sandbox_id":1})
        .limit(201)
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(baseline.len(), 201);
    assert_eq!(take_counts(&commands), (1, 1));

    // Exercise the default page, the first size crossing the initial-batch
    // boundary, and the maximum. Cursor traversal must neither lose the
    // lookahead row nor repeat it, and must stop at the partial final page.
    for limit in [50_u16, 101, 200] {
        let mut cursor = None;
        let mut offset = 0;
        loop {
            let page = store
                .page(
                    &report,
                    PageQuery {
                        limit: Some(limit),
                        cursor,
                        sort: Sort::Name,
                        order: Order::Asc,
                    },
                )
                .await
                .unwrap();
            assert_eq!(take_counts(&commands), (1, 0), "page limit {limit}");
            let end = (offset + usize::from(limit)).min(rows.len());
            assert_eq!(
                serde_json::to_value(&page.items).unwrap(),
                serde_json::to_value(&rows[offset..end]).unwrap(),
                "page limit {limit}, offset {offset}"
            );
            assert_eq!(page.total_count, rows.len() as u64);
            assert_eq!(page.next.is_some(), end < rows.len());
            offset = end;
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(offset, rows.len());
    }
    println!("REPORT_PAGE_BATCH rows=225 limits=50,101,200 baseline_find=1 baseline_getMore=1 page_find=1 page_getMore=0 cursor_parity=PASS");
    store.db.drop().await.unwrap();
}
