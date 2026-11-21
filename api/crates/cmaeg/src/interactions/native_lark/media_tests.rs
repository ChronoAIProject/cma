use super::{fixture::*, *};
use cmaeg_core::channel::LarkRegion;

#[tokio::test]
async fn legacy_native_cards_keep_identical_serialization_and_custody_without_image_authority() {
    let f = Fixture::new(false, true, LarkRegion::Lark).await;
    let card = f.card(false, true).await;
    let value = serde_json::to_value(&card.card).unwrap();
    assert!(value["native_lark"].get("image").is_none());
    assert!(value["native_lark"].get("images").is_none());
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
    let card = render_with_images(
        &text,
        "00000000000040008000000000000000",
        1,
        false,
        std::slice::from_ref(&image),
    )
    .unwrap()
    .0;
    assert_eq!(card["body"]["elements"][0]["tag"], "img");
    assert!(Image::confirmed("key with spaces".into()).is_err());
    assert!(render_with_images(
        &outbound(true),
        "00000000000040008000000000000000",
        1,
        true,
        std::slice::from_ref(&image)
    )
    .is_err());
    let mut too_large = text;
    too_large.text = Some("x".repeat(4001));
    assert!(render_with_images(
        &too_large,
        "00000000000040008000000000000000",
        1,
        false,
        std::slice::from_ref(&image)
    )
    .is_err());
}

#[tokio::test]
async fn historical_scalar_image_keeps_its_exact_serialization_and_custody() {
    let f = Fixture::new(false, false, LarkRegion::Feishu).await;
    let mut prepared = f.card(false, true).await;
    let authority = prepared.card.native_lark.as_mut().unwrap();
    authority.image = Some(Image::confirmed("img_historical".into()).unwrap());
    let custody = prepared
        .card
        .native_lark
        .as_ref()
        .unwrap()
        .digest(&prepared.card, &f.channel)
        .unwrap();
    prepared.card.native_lark.as_mut().unwrap().custody = custody.clone();
    let encoded = serde_json::to_vec(&prepared.card).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert!(value["native_lark"].get("images").is_none());
    assert_eq!(
        value["native_lark"]["image"],
        serde_json::json!({"key":"img_historical"})
    );
    // An old scalar record has no new field. Reading it cannot rewrite its shape or proof.
    let restored: Card = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(serde_json::to_vec(&restored).unwrap(), encoded);
    assert_eq!(restored.native_lark.as_ref().unwrap().custody, custody);
    assert!(restored
        .native_lark
        .as_ref()
        .unwrap()
        .current(&restored, &f.channel));
    let mut injected = value;
    injected["native_lark"]["images"] = serde_json::json!([{"key":"img_1"},{"key":"img_2"}]);
    let changed: Card = serde_json::from_value(injected).unwrap();
    assert!(markup(&changed).is_err());
    assert!(!changed
        .native_lark
        .as_ref()
        .unwrap()
        .current(&changed, &f.channel));
}

#[test]
fn native_multi_image_preflight_reserves_all_elements_and_key_bytes_before_upload() {
    let mut message = outbound(false);
    // 96 text elements + two controls = 98. One image fits, four do not.
    let fields = (0..15)
        .map(|_| serde_json::json!({"label":"l","value":"v"}))
        .collect::<Vec<_>>();
    let card = serde_json::json!({"type":"card","title":"t","fields":fields});
    message.text = None;
    let controls = message.blocks.remove(0);
    message.blocks = (0..6)
        .map(|_| serde_json::from_value(card.clone()).unwrap())
        .collect();
    message.blocks.push(controls);
    let image = crate::native_lark::outbound::tests::fixture::attachment("image/png");
    message.native_attachments = vec![image.clone()];
    assert!(preflight_media(&message, false).is_ok());
    message.native_attachments = vec![image; 4];
    assert!(preflight_media(&message, true).is_err());
    let images: Vec<_> = (0..4)
        .map(|i| Image::confirmed(format!("img_{i}")).unwrap())
        .collect();
    assert!(render_with_images(
        &outbound(true),
        "00000000000040008000000000000000",
        1,
        true,
        &images
    )
    .is_err());
    let too_many = vec![Image::confirmed("img".into()).unwrap(); 5];
    assert!(render_with_images(
        &outbound(false),
        "00000000000040008000000000000000",
        1,
        false,
        &too_many
    )
    .is_err());
}
