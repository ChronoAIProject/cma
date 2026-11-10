use super::{revocation_tests::*, *};

#[tokio::test]
async fn default_writer_refuses_all_mutations_past_either_old_limit() {
    for byte_saturated in [false, true] {
        let (enabled, rows, id) = fixture().await;
        let owner = caller("owner");
        let token = "cd".repeat(32);
        let link = enabled
            .create_link(
                &owner,
                &id,
                "link",
                CreateDocumentLink {
                    expected_revision: 2,
                    permission: DocumentPermission::Read,
                    token: token.clone(),
                },
            )
            .await
            .unwrap()
            .link
            .unwrap();
        let comment = enabled
            .comment(
                &owner,
                &id,
                None,
                "comment",
                CreateDocumentComment {
                    body: "x".into(),
                    document_revision: None,
                },
            )
            .await
            .unwrap()
            .comment
            .unwrap();
        let revision = fill_receipts(&rows, &id).await;
        if byte_saturated {
            fill_metadata(&rows, &id).await;
        }
        let before = serde_json::to_vec(&rows.get(&id).await.unwrap().unwrap()).unwrap();
        let rollback = DocumentService::new(rows.clone(), enabled.blobs.clone());
        assert!(!rollback.revocation_reserve_enabled);
        let commands = [
            mutation::Command::Update(UpdateDocument {
                expected_revision: revision,
                title: Some("Changed".into()),
                markdown: None,
            }),
            mutation::Command::Grant(
                "new-reader".into(),
                GrantDocument {
                    expected_revision: revision,
                    permission: DocumentPermission::Read,
                },
            ),
            mutation::Command::Revoke("guest".into(), revision),
            mutation::Command::Link(CreateDocumentLink {
                expected_revision: revision,
                permission: DocumentPermission::Read,
                token: "ef".repeat(32),
            }),
            mutation::Command::RevokeLink(link.id, revision),
            mutation::Command::Comment(CreateDocumentComment {
                body: "New comment".into(),
                document_revision: None,
            }),
            mutation::Command::RemoveComment(comment.id, revision),
        ];
        for command in commands {
            assert_eq!(
                rollback
                    .mutate_guarded(&owner, &id, None, "new-operation", command, None)
                    .await,
                Err(DocumentError::Capacity)
            );
        }
        assert_eq!(
            serde_json::to_vec(&rows.get(&id).await.unwrap().unwrap()).unwrap(),
            before
        );
        assert!(rollback.source(&owner, &id, None, None).await.is_ok());
        assert!(rollback
            .source(&caller("guest"), &id, None, None)
            .await
            .is_ok());
        assert_eq!(
            rollback
                .revoke(&caller("foreign"), &id, "deny", "guest", revision)
                .await,
            Err(DocumentError::NotFound)
        );
    }
}

#[tokio::test]
async fn default_writer_allows_revoke_within_old_limits_and_denies_restoration() {
    let (enabled, rows, id) = fixture().await;
    let rollback = DocumentService::new(rows.clone(), enabled.blobs.clone());
    let revoked = rollback
        .revoke(&caller("owner"), &id, "revoke", "guest", 2)
        .await
        .unwrap();
    assert_eq!(
        rollback
            .revoke(&caller("owner"), &id, "revoke", "guest", 2)
            .await,
        Ok(revoked)
    );
    assert_eq!(
        rollback.source(&caller("guest"), &id, None, None).await,
        Err(DocumentError::NotFound)
    );
    assert!(rollback
        .source(&caller("owner"), &id, None, None)
        .await
        .is_ok());
    let record = rows.get(&id).await.unwrap().unwrap();
    assert!(record.receipts.len() <= MAX_RECEIPTS);
    assert!(record.metadata_bytes().unwrap() <= MAX_METADATA_BYTES);
}
