use super::*;

#[test]
fn telegram_raw_nontext_with_text_never_becomes_message_or_control_input() {
    for platform in [BridgePlatform::Telegram, BridgePlatform::TelegramNew] {
        for case in event_cases::cases(&platform)
            .into_iter()
            .filter(|case| matches!(case, event_cases::EventCase::RawNontext(_)))
        {
            let mut fixture = Fixture::new(platform.clone());
            event_cases::text(
                &mut fixture.body,
                &platform,
                "CMA-CONTROL/1 cancel ccr_source",
            );
            case.apply(&mut fixture.body, &platform);
            let write = fixture.write().unwrap();
            assert_eq!(
                write.record().admission,
                InboxAdmission::PartitionDenied {
                    reason: PartitionRefusal::ContentUnsupported
                },
                "{platform:?} {case:?}"
            );
            assert!(write.control_refusal().is_none());
        }
    }
}

#[test]
fn signed_unsupported_event_matrix_never_establishes_message_or_control_authority() {
    for platform in event_cases::platforms() {
        for case in event_cases::cases(&platform) {
            for text in ["hello", "CMA-CONTROL/1 cancel ccr_source"] {
                let mut fixture = Fixture::new(platform.clone());
                event_cases::text(&mut fixture.body, &platform, text);
                fixture.body["thread_id"] = json!("interaction:app:matrix-private-context");
                case.apply(&mut fixture.body, &platform);
                let result = fixture.write();
                let Some(reason) = case.expected(&platform) else {
                    assert!(
                        matches!(result, Err(CallbackError::Payload)),
                        "{platform:?} {case:?}"
                    );
                    continue;
                };
                let write = result.unwrap();
                assert_eq!(
                    write.record().admission,
                    InboxAdmission::PartitionDenied { reason },
                    "{platform:?} {case:?} {text}"
                );
                assert!(write.control_refusal().is_none(), "{platform:?} {case:?}");
                assert!(write.reply_token().is_none());
                assert!(write.thread_context().is_none());
                assert!(write.record().reply_expires_at.is_none());
                assert!(write.record().execution_hold.is_none());
                let encoded = serde_json::to_string(write.record()).unwrap();
                assert!(!encoded.contains("matrix-private-context"));
                assert!(!encoded.contains("raw_platform_data"));
                let retained: StoredInbox = serde_json::from_str(&encoded).unwrap();
                assert_eq!(retained.admission, write.record().admission);
                assert_eq!(
                    retained.capabilities.as_ref().unwrap().contract(),
                    CapabilityContract::NyxidPlatformV1
                );
                let source =
                    NormalizedPartitionSource::from_stored(&retained, &fixture.bridge).unwrap();
                assert_eq!(
                    source.admit(fixture.config()).unwrap(),
                    PartitionAdmissionDecision::Refused {
                        reason: PartitionRefusal::UpstreamDenied
                    }
                );
                let signed = fixture.signed().unwrap();
                assert!(crate::bridge::ingress::redelivery::observe(
                    &signed.authenticated_payload(&fixture.bytes()).unwrap()
                )
                .unwrap()
                .is_none());
            }
        }
    }
}
