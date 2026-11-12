use super::*;

#[test]
fn agent_indexes_include_the_live_worktree_constraint() {
    let indexes = agent_indexes();
    let named = |name: &str| {
        indexes
            .iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some(name)
            })
            .expect("required named Agent index")
    };
    assert_eq!(
        indexes[0].keys,
        doc! { "owner": 1, "status": 1, "updated_at": -1 }
    );
    assert_eq!(indexes[1].keys, doc! { "owner": 1, "creation_key": 1 });
    let creation_options = indexes[1].options.as_ref().unwrap();
    assert_eq!(creation_options.unique, Some(true));
    assert_eq!(
        creation_options.name.as_deref(),
        Some("agents_owner_creation_key")
    );
    assert_eq!(indexes[2].keys, doc! { "sandbox_id": 1 });
    assert_eq!(indexes[3].keys, doc! { "workspace_id": 1, "status": 1 });
    assert_eq!(
        indexes[4].keys,
        doc! {
            "work_pending": 1,
            "work_ready_at": 1,
            "work_lease_expires_at": 1,
            "_id": 1,
        }
    );
    assert_eq!(
        indexes[4].options.as_ref().unwrap().name.as_deref(),
        Some("agents_due_work")
    );
    let pacing = named("agents_paced_work");
    assert_eq!(
        pacing.keys,
        doc! { "work_pending": 1, "worker_failure_pacing_v1.until": 1,
        "work_lease_expires_at": 1, "_id": 1 }
    );
    assert_eq!(
        pacing.options.as_ref().unwrap().partial_filter_expression,
        Some(doc! { "worker_failure_pacing_v1.until": { "$exists": true } })
    );
    assert_eq!(
        named("agents_live_connections").keys,
        doc! { "status": 1, "_id": 1 }
    );
    let worktree = named("agents_live_worktree");
    assert_eq!(worktree.keys, doc! { "worktree_id": 1 });
    let options = worktree.options.as_ref().unwrap();
    assert_eq!(options.unique, Some(true));
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! { "status": { "$in": LIVE_STATUSES } })
    );
}
