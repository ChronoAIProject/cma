use super::{allocated::request, retained_views, *};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::{collections::BTreeSet, sync::Mutex, time::Instant};

type NameQueries = Arc<Mutex<Vec<Document>>>;

async fn observed() -> (Arc<ReportingStore>, NameQueries) {
    let fixture = retained_views::enabled().await;
    let queries = NameQueries::default();
    let captured = queries.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.max_pool_size = Some(4);
    // Install after diagnostics, which deliberately clears full command hooks.
    super::super::budget::observe(&mut options, fixture.circuit.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" && event.command.get_str("find").ok() == Some("users") {
                captured.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let mut store = (*fixture).clone();
    store.persistence = store.persistence.with_client(client);
    (Arc::new(store), queries)
}

pub(super) async fn name(store: &ReportingStore, subject: &str, name: &str) {
    store
        .collection("users")
        .update_one(
            doc! {"_id":subject},
            doc! {"$set":{
                "document_schema_version":super::super::super::schema::DOCUMENT_SCHEMA_VERSION,
                "display_name":name,
                "avatar_url":"private-fixture-field",
                "first_seen_at":"private-fixture-field"
            }},
        )
        .upsert(true)
        .await
        .unwrap();
}

async fn get(store: &Arc<ReportingStore>, path: &str) -> (u16, serde_json::Value) {
    request(
        store,
        "GET",
        path,
        "operator",
        true,
        serde_json::Value::Null,
    )
    .await
}

async fn retained(store: &Arc<ReportingStore>) -> Report {
    retained_views::build(
        store,
        &scope("operator", true),
        &ViewInput::default().validate(now()).unwrap(),
    )
    .await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_report_labels_follow_directory_changes_without_mutating_snapshot() {
    let (store, _) = observed().await;
    let state = state(0, now() - 3600);
    seed(&store, std::slice::from_ref(&state)).await;
    name(&store, "owner-0", "  Ada Lovelace  ").await;
    let report = retained(&store).await;
    let root = format!("/api/v1/admin/usage/metrics/reports/{}", report.report_id);
    let page = format!("{root}/sandboxes");
    let detail = format!("{page}/{}", state.sandbox_id);
    let owners = format!("{root}/owners");
    for (path, pointer) in [
        (&page, "/items/0/owner/display_name"),
        (&detail, "/owner/display_name"),
        (&owners, "/items/0/display_name"),
    ] {
        let (status, body) = get(&store, path).await;
        assert_eq!(status, 200);
        assert_eq!(body.pointer(pointer).unwrap(), "Ada Lovelace");
        assert!(!body.to_string().contains("private-fixture-field"));
    }
    name(&store, "owner-0", "Grace Hopper").await;
    let (status, renamed) = get(&store, &page).await;
    assert_eq!(status, 200);
    assert_eq!(renamed["report_id"], report.report_id);
    assert_eq!(renamed["items"][0]["owner"]["display_name"], "Grace Hopper");
    for invalid in [
        "",
        "  ",
        "owner-0",
        "sbx_00000000000000000000000000000001",
        "Ada\nLovelace",
    ] {
        name(&store, "owner-0", invalid).await;
        let (status, body) = get(&store, &detail).await;
        assert_eq!(status, 200);
        assert!(body["owner"]["display_name"].is_null());
    }
    store
        .collection("users")
        .delete_many(doc! {})
        .await
        .unwrap();
    let (status, removed) = get(&store, &page).await;
    assert_eq!(status, 200);
    assert!(removed["items"][0]["owner"]["display_name"].is_null());
    let stored = store
        .collection(ROWS)
        .find_one(doc! {"report_id":&report.report_id})
        .await
        .unwrap()
        .unwrap();
    assert!(decode::<Row>(&stored).unwrap().owner.display_name.is_none());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn legacy_owner_and_admin_pages_include_current_public_names() {
    let store = fixture().await;
    seed(&store, &[state(0, now() - 3600), state(1, now() - 3600)]).await;
    name(&store, "owner-0", "Ada Lovelace").await;
    name(&store, "owner-1", "Grace Hopper").await;
    for (admin, count) in [(false, 1), (true, 2)] {
        let report = store
            .submit(
                scope("owner-0", admin),
                ReportInput::default().validate(now()).unwrap(),
                now(),
            )
            .await
            .unwrap();
        let lease = store
            .claim_report("report:0", "name-worker")
            .await
            .unwrap()
            .unwrap();
        let generated =
            crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
                .await
                .unwrap();
        store.finish_report(&lease, generated, true).await.unwrap();
        let prefix = if admin {
            "/api/v1/admin/usage/metrics/reports"
        } else {
            "/api/v1/usage/metrics/reports"
        };
        let path = format!("{prefix}/{}/sandboxes", report.report_id);
        let (status, page) = request(
            &store,
            "GET",
            &path,
            "owner-0",
            admin,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        let rows = page["items"].as_array().unwrap();
        assert_eq!(rows.len(), count);
        assert!(rows
            .iter()
            .all(|row| row["owner"]["display_name"].is_string()));
        let (status, detail) = request(
            &store,
            "GET",
            &format!("{path}/{}", rows[0]["sandbox_id"].as_str().unwrap()),
            "owner-0",
            admin,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            detail["owner"]["display_name"],
            rows[0]["owner"]["display_name"]
        );
    }
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn owner_names_read_one_bounded_public_projection_after_page_selection() {
    let (store, queries) = observed().await;
    let mut states = Vec::new();
    for n in 0..201 {
        let mut state = state(n, now() - 3600);
        state.owner = SubjectId(format!("owner-{n:03}"));
        name(&store, state.owner.as_str(), &format!("Public person {n}")).await;
        states.push(state);
    }
    seed(&store, &states).await;
    let report = retained(&store).await;
    let root = format!("/api/v1/admin/usage/metrics/reports/{}", report.report_id);
    for suffix in ["sandboxes", "sandboxes?limit=200", "owners?limit=200"] {
        queries.lock().unwrap().clear();
        let started = Instant::now();
        let (status, page) = get(&store, &format!("{root}/{suffix}")).await;
        let elapsed = started.elapsed().as_micros();
        assert_eq!(status, 200);
        let items = page["items"].as_array().unwrap();
        let expected = if suffix == "sandboxes" { 50 } else { 200 };
        assert_eq!(items.len(), expected);
        assert!(page["next"].is_string());
        let owners: Vec<_> = items
            .iter()
            .map(|item| {
                if suffix.starts_with("owners") {
                    item
                } else {
                    &item["owner"]
                }
            })
            .collect();
        assert!(owners.iter().all(|owner| owner["display_name"].is_string()));
        let subjects: BTreeSet<_> = owners
            .iter()
            .map(|owner| owner["subject"].as_str().unwrap().to_owned())
            .collect();
        let captured = queries.lock().unwrap().clone();
        assert_eq!(captured.len(), 1, "one users query per returned page");
        let query = &captured[0];
        let requested = query
            .get_document("filter")
            .unwrap()
            .get_document("_id")
            .unwrap()
            .get_array("$in")
            .unwrap();
        assert_eq!(requested.len(), expected);
        assert_eq!(
            requested
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>(),
            subjects
        );
        assert_eq!(
            query.get_document("projection").unwrap(),
            &doc! {"_id":1,"document_schema_version":1,"display_name":1}
        );
        assert!(!page.to_string().contains("private-fixture-field"));
        eprintln!("owner_names route={suffix} rows={expected} complete_body_us={elapsed} bytes={} users_find=1", serde_json::to_vec(&page).unwrap().len());
    }
    queries.lock().unwrap().clear();
    let (status, page) = get(&store, &format!("{root}/owners?search=absent")).await;
    assert_eq!(status, 200);
    assert!(page["items"].as_array().unwrap().is_empty());
    assert_eq!(
        get(&store, &format!("{root}/sandboxes?limit=201")).await.0,
        400
    );
    assert!(queries.lock().unwrap().is_empty());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn owner_names_preserve_authority_and_fail_closed_on_directory_schema() {
    let (store, queries) = observed().await;
    seed(&store, &[state(0, now() - 3600)]).await;
    name(&store, "owner-0", "Ada Lovelace").await;
    let report = retained(&store).await;
    let path = format!(
        "/api/v1/admin/usage/metrics/reports/{}/sandboxes",
        report.report_id
    );
    for (principal, capability, expected) in [("foreign", true, 404), ("operator", false, 403)] {
        let (status, _) = request(
            &store,
            "GET",
            &path,
            principal,
            capability,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, expected);
    }
    store
        .collection("sandbox_usage")
        .update_one(
            doc! {"_id":format!("sbx_{:032x}",0)},
            doc! {"$set":{"owner":"transferred"}},
        )
        .await
        .unwrap();
    assert_eq!(get(&store, &path).await.0, 404);
    assert!(queries.lock().unwrap().is_empty());
    store
        .collection("sandbox_usage")
        .update_one(
            doc! {"_id":format!("sbx_{:032x}",0)},
            doc! {"$set":{"owner":"owner-0"}},
        )
        .await
        .unwrap();
    store
        .collection("users")
        .update_one(
            doc! {"_id":"owner-0"},
            doc! {"$set":{"document_schema_version":999_i64}},
        )
        .await
        .unwrap();
    let (status, body) = get(&store, &path).await;
    assert_eq!(status, 503);
    assert!(body.get("items").is_none());
    assert_eq!(queries.lock().unwrap().len(), 1);
    name(&store, "owner-0", "Ada Lovelace").await;
    assert_eq!(get(&store, &path).await.0, 200);
    store.db.drop().await.unwrap();
}
