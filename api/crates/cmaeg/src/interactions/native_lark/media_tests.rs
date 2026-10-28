use super::{fixture::*, *};
use cmaeg_core::channel::LarkRegion;

#[tokio::test]
async fn legacy_native_cards_keep_identical_serialization_and_custody_without_image_authority() {
    let f = Fixture::new(false, true, LarkRegion::Lark).await;
    let card = f.card(false, true).await;
    let value = serde_json::to_value(&card.card).unwrap();
    assert!(value["native_lark"].get("image").is_none());
    let restored: Card = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), value);
    assert!(restored
        .native_lark
        .as_ref()
        .unwrap()
        .current(&restored, &f.channel));
    assert_eq!(
        markup(&restored).unwrap().0,
        render(&restored.message, &restored.id, restored.revision, false)
            .unwrap()
            .0
    );
    // An injected image key has no authority unless it was included in custody before binding.
    let mut changed = value;
    changed["native_lark"]["image"] = serde_json::json!({"key":"img_unattested"});
    let changed: Card = serde_json::from_value(changed).unwrap();
    assert!(!changed
        .native_lark
        .as_ref()
        .unwrap()
        .current(&changed, &f.channel));
}

#[test]
fn native_image_renderer_reserves_a_real_component_and_preserves_existing_card_limits() {
    let image = Image::confirmed("img_confirmed".into()).unwrap();
    let text = outbound(false);
    let card = render_with_image(
        &text,
        "00000000000040008000000000000000",
        1,
        false,
        Some(&image),
    )
    .unwrap()
    .0;
    assert_eq!(card["body"]["elements"][0]["tag"], "img");
    assert!(Image::confirmed("key with spaces".into()).is_err());
    assert!(render_with_image(
        &outbound(true),
        "00000000000040008000000000000000",
        1,
        true,
        Some(&image)
    )
    .is_err());
    let mut too_large = text;
    too_large.text = Some("x".repeat(4001));
    assert!(render_with_image(
        &too_large,
        "00000000000040008000000000000000",
        1,
        false,
        Some(&image)
    )
    .is_err());
}
