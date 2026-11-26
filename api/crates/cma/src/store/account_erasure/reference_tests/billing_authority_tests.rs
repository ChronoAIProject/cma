use super::*;
use layer2_core::{
    budget::{BudgetPolicy, OwnerBudgetPolicy},
    PlatformSettings, PlatformSettingsStore,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn settings_replacement_and_owner_redaction_preserve_billing_authority_pair() {
    let db = fixture().await;
    let authority = crate::store::billing_authority::format_v1::proposed_documents();
    db.collection("platform_settings")
        .insert_many(authority.clone())
        .await
        .unwrap();
    let store = crate::store::MongoPlatformSettingsStore::open(db.client(), db.name())
        .await
        .unwrap();
    let mut settings = PlatformSettings::default();
    settings.model_budget.owners.push(OwnerBudgetPolicy {
        owner: SubjectId::new("erased"),
        policy: BudgetPolicy {
            owner_day_limit: 100,
            turn_limit: 10,
            max_calls_per_turn: 10,
            day_timezone: "UTC".into(),
            input_token_units: 1,
            output_token_units: 1,
            max_unobserved_units: 1,
            usage_timeout_seconds: 30,
        },
    });
    store.replace(0, &settings).await.unwrap();
    assert!(store.replace(0, &settings).await.is_err());
    let platform = row(&db, "platform_settings", "platform").await;
    assert_eq!(
        redact(&db, "platform_settings", &platform, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = store.read().await.unwrap();
    assert_eq!(cleaned.revision, 2);
    assert!(cleaned.settings.model_budget.owners.is_empty());
    for original in authority {
        assert_eq!(
            redact(&db, "platform_settings", &original, &subjects())
                .await
                .unwrap(),
            Redaction::Unchanged
        );
        assert_eq!(
            row(&db, "platform_settings", original.get_str("_id").unwrap()).await,
            original
        );
    }
    crate::store::billing_authority::require_legacy_compatible(&db)
        .await
        .unwrap();
    db.drop().await.unwrap();
}
