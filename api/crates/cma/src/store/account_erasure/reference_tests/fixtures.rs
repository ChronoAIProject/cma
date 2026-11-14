use super::*;
use layer2_core::resource_access::*;
use layer2_core::AgentId;

pub(super) const AT: &str = "2026-11-14T00:00:00Z";
pub(super) fn document() -> StoredDocument {
    let id = "doc_11111111111111111111111111111111".to_owned();
    let sha = digest(b"# Source");
    let metadata = DocumentView {
        id: id.clone(),
        owner_subject: "survivor".into(),
        title: "Surviving title".into(),
        revision: 3,
        source_revision: 1,
        sha256: sha.clone(),
        size_bytes: 8,
        created_at: AT.into(),
        updated_at: AT.into(),
        origin_agent_id: None,
        access: DocumentAccess::Owner,
    };
    let comment = DocumentComment {
        id: "dcm_22222222222222222222222222222222".into(),
        author_subject: "erased".into(),
        body: "Private comment".into(),
        document_revision: 1,
        created_at: AT.into(),
    };
    let other = DocumentComment {
        id: "dcm_33333333333333333333333333333333".into(),
        author_subject: "other".into(),
        body: "Keep this comment".into(),
        document_revision: 1,
        created_at: AT.into(),
    };
    let value = StoredDocument {
        runtime_author_agent_id: None,
        metadata: metadata.clone(),
        title_search: metadata.title.to_lowercase(),
        revisions: vec![StoredRevision {
            metadata: DocumentRevision {
                revision: 1,
                title: metadata.title.clone(),
                sha256: sha.clone(),
                size_bytes: 8,
                created_at: AT.into(),
                author_subject: "survivor".into(),
            },
            object_key: format!("documents/{}/{id}/{sha}.md", digest(b"survivor")),
        }],
        shares: vec![
            DocumentShare {
                subject: "erased".into(),
                permission: DocumentPermission::Comment,
            },
            DocumentShare {
                subject: "other".into(),
                permission: DocumentPermission::Comment,
            },
        ],
        links: vec![],
        comments: vec![comment.clone(), other],
        receipts: BTreeMap::from([(
            "comment-receipt".into(),
            DocumentReceipt {
                fingerprint: digest(b"comment"),
                outcome: DocumentMutation {
                    document: metadata,
                    comment: Some(comment),
                    link: None,
                },
            },
        )]),
        creation_fingerprint: digest(b"create"),
    };
    value.validate().unwrap();
    value
}
pub(in crate::store::account_erasure) fn share() -> Document {
    let record = ResourceShare {
        id: "share-one".into(),
        owner: ResourceOwner::Person {
            subject: SubjectId::new("survivor"),
        },
        selector: ResourceSelector::Agent {
            agent_id: AgentId("agt-other".into()),
        },
        target_org_id: SubjectId::new(""),
        link: Some(AgentShareLink {
            audience: AgentShareAudience::Public,
            token_sha256: digest(b"link"),
            accepted_subjects: vec![SubjectId::new("erased"), SubjectId::new("other")],
        }),
        mode: ShareMode::View,
        issuer_actor: ResourceActor::Human {
            subject: SubjectId::new("survivor"),
        },
        created_at: AT.into(),
        revoked_at: None,
        revision: 1,
    };
    assert!(record.validate());
    doc! {"_id":&record.id,"document_schema_version":1_i64,"sandbox_id":"sbx-other","selector_kind":"agent","selector_id":"agt-other","target_org_id":"","revoked":false,"revision":1_i64,"accepted_subjects":["erased","other"],"record":mongodb::bson::serialize_to_bson(&record).unwrap()}
}
pub(super) fn revision() -> (Document, TriggerRevision, TriggerId) {
    let content:TriggerContent = serde_json::from_value(serde_json::json!({
        "presentation":{"title":"Publisher secret","description":"Private description","button_label":"run_with_cma","theme":"auto","signup_hint":"Private display"},
        "target":{"inline":{"agent":{"reasoning_effort":"high","permission_mode":"autonomous","system_prompt":"Publisher prompt"}}},
        "reuse":{"sandbox":"always_new","workspace":"always_new","agent":"always_new"},
        "first_message":"Publisher first message","quick_messages":[{"name":"Private name","description":"Private description","text":"Private body"}],"consent":{"always_review":true}
    })).unwrap();
    let revision = TriggerRevision::new(1, content, AT.into()).unwrap();
    let id = TriggerId("trg_erasure".into());
    let row = doc! {"_id":"trg_erasure:0000000001","document_schema_version":1_i64,"trigger_id":id.as_str(),"public_id":"pub_erasure","owner":"erased","revision":1_i64,"sandbox_profile_id":"","workspace_profile_id":"","agent_profile_id":"","published_at":AT,"record":mongodb::bson::serialize_to_bson(&revision).unwrap()};
    (row, revision, id)
}
pub(super) fn launch(trigger_id: &TriggerId, revision_digest: &str) -> Launch {
    Launch::new(LaunchSeed {
        trigger_id: trigger_id.clone(),
        revision_digest: revision_digest.into(),
        public_id: PublicTriggerId("pub_erasure".into()),
        owner: SubjectId::new("survivor"),
        authority: LaunchAuthorityClass::Visitor,
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: vec![],
            checked_at: AT.into(),
        },
        review: ReviewRequirements {
            needs: vec![],
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: true,
        command_key_hash: digest(b"launch-key"),
        command_fingerprint: digest(b"launch-fingerprint"),
        created_at: AT.into(),
    })
    .unwrap()
}
pub(super) fn encode_launch(launch: &Launch) -> Document {
    let mut row = doc! {"_id":launch.id.as_str(),"document_schema_version":1_i64,"owner":launch.owner.as_str(),"trigger_id":launch.trigger_id.as_str(),"public_id":launch.public_id.as_str(),"command_key_hash":&launch.command_key_hash,"command_fingerprint":&launch.command_fingerprint,"created_at":&launch.created_at,"work_ready_at":&launch.work.ready_at,"work_lease_holder":null,"work_lease_fence":null,"work_lease_expires_at":null,"ready_at":null,"event_head":0_i64,"revision":1_i64,"mutation_version":1_i64,"record":mongodb::bson::serialize_to_bson(launch).unwrap()};
    row.insert(
        "state",
        mongodb::bson::serialize_to_bson(&launch.state).unwrap(),
    );
    row.insert(
        "blocked_reason",
        mongodb::bson::serialize_to_bson(&launch.blocked_reason).unwrap(),
    );
    row.insert(
        "work_step",
        mongodb::bson::serialize_to_bson(&launch.work.step).unwrap(),
    );
    row
}
