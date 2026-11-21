use super::{discord_form, discord_form_values};
use crate::discord::modal_fixture::{message, values};
use cmaeg_core::message::{Block, FormFieldKind};
use serde_json::{json, Value};

#[test]
fn discord_modal_fields_are_canonical_and_keep_private_business_ids() {
    let decoded = discord_form_values::submitted(&message(), &values()).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        json!({"private-text":"Ada","private-choice":"private-write","private-number":12.5,"private-date":"2028-02-29"})
    );
    let mut optional = values();
    optional[1]["component"]["values"] = json!([]);
    assert!(!discord_form_values::submitted(&message(), &optional)
        .unwrap()
        .contains_key("private-choice"));
    optional.as_array_mut().unwrap().remove(1);
    assert!(discord_form_values::submitted(&message(), &optional).is_ok());
    let mut unicode = values();
    unicode[0]["component"]["value"] = "🌍".repeat(1000).into();
    assert!(discord_form_values::submitted(&message(), &unicode).is_ok());
    unicode[0]["component"]["value"] = "🌍".repeat(1001).into();
    assert!(discord_form_values::submitted(&message(), &unicode).is_err());
}
#[test]
fn discord_modal_number_date_and_shapes_are_strictly_validated() {
    for invalid in [
        "NaN", "Infinity", " 1", "1 ", "+1", "01", "1_000", "1,000", "", "1e999",
    ] {
        let mut payload = values();
        payload[2]["component"]["value"] = invalid.into();
        assert!(
            discord_form_values::submitted(&message(), &payload).is_err(),
            "{invalid}"
        );
    }
    for invalid in [
        "2027-02-29",
        "2028-2-29",
        "2028-02-30",
        "2028-13-01",
        "2028-02-29Z",
        " 2028-02-29",
        "29/02/2028",
        "",
    ] {
        let mut payload = values();
        payload[3]["component"]["value"] = invalid.into();
        assert!(
            discord_form_values::submitted(&message(), &payload).is_err(),
            "{invalid}"
        );
    }
    for (pointer, value) in [
        ("/0/type", json!(1)),
        ("/0/component/type", json!(3)),
        ("/0/component/custom_id", json!("f00")),
        ("/0/component/custom_id", json!("private-text")),
        ("/0/component/custom_id", json!("f9")),
        ("/1/component/custom_id", json!("f0")),
        ("/1/component/values", json!(["private-write"])),
        ("/1/component/values", json!(["01"])),
        ("/1/component/values", json!(["0", "1"])),
        ("/1/component/values", json!(["2"])),
        ("/0/component/value", Value::Null),
        ("/0/component/value", json!("")),
    ] {
        let mut payload = values();
        *payload.pointer_mut(pointer).unwrap() = value;
        assert!(
            discord_form_values::submitted(&message(), &payload).is_err(),
            "{pointer}"
        );
    }
    let mut absent = values();
    absent.as_array_mut().unwrap().remove(0);
    assert!(discord_form_values::submitted(&message(), &absent).is_err());
    let mut extra = values();
    let duplicate = extra[0].clone();
    extra.as_array_mut().unwrap().push(duplicate);
    assert!(discord_form_values::submitted(&message(), &extra).is_err());
}
#[test]
fn discord_modal_native_eligibility_is_bounded_without_changing_neutral_forms() {
    let base = message();
    for count in [1, 5, 6] {
        let mut candidate = base.clone();
        let Block::Form { fields, .. } = &mut candidate.blocks[0] else {
            panic!()
        };
        let field = fields[0].clone();
        *fields = (0..count)
            .map(|i| {
                let mut f = field.clone();
                f.id = format!("f{i}");
                f
            })
            .collect();
        candidate.validate().unwrap();
        assert_eq!(discord_form::schema(&candidate).is_ok(), count <= 5);
    }
    for chars in [45, 46] {
        let mut candidate = base.clone();
        let Block::Form { fields, .. } = &mut candidate.blocks[0] else {
            panic!()
        };
        fields[0].label = "界".repeat(chars);
        assert_eq!(discord_form::schema(&candidate).is_ok(), chars == 45);
    }
    let mut mixed = base.clone();
    mixed
        .blocks
        .extend(crate::discord::components_fixture::message(false).blocks);
    assert!(discord_form::schema(&mixed).is_err());
    let mut duplicate = base.clone();
    duplicate.blocks.push(base.blocks[0].clone());
    assert!(discord_form::schema(&duplicate).is_err());
    let mut duplicates = base;
    let Block::Form { fields, .. } = &mut duplicates.blocks[0] else {
        panic!()
    };
    let FormFieldKind::Choice { options } = &mut fields[1].kind else {
        panic!()
    };
    options[1].id = options[0].id.clone();
    assert!(discord_form::schema(&duplicates).is_err());
}

#[tokio::test]
async fn discord_modal_attempt_expiry_denies_fresh_submit_but_preserves_exact_consumed_recovery() {
    use crate::discord::{
        components_fixture::FixtureState,
        modal_fixture::{card, launch, restored, submit},
    };
    for consumed in [false, true] {
        let f = FixtureState::new().await;
        let card = card(&f).await;
        let (_, custom) = launch(&f, &card, 2).await;
        let raw = submit(&f, &custom);
        if consumed {
            f.setup.fixture.fail(&f.id(&raw), false);
            assert!(f.receive(raw.clone(), 3).await.is_err());
        }
        let (mut saved, version) = f.store.load(&card.card.id).await.unwrap();
        saved.discord_form_launch.as_mut().unwrap().expires_at = super::card::now() - 1;
        f.store.save(&saved, Some(version)).await.unwrap();
        f.receive(raw.clone(), 4).await.unwrap();
        assert_eq!(
            f.store
                .load(&card.card.id)
                .await
                .unwrap()
                .0
                .consumed
                .is_some(),
            consumed
        );
        if consumed {
            assert_eq!(
                restored(&f, &raw).await.request.conversation_id,
                f.original.conversation_id
            );
        } else {
            assert!(!f
                .setup
                .fixture
                .memory
                .rows
                .lock()
                .unwrap()
                .contains_key(&f.id(&raw)));
        }
    }
}
