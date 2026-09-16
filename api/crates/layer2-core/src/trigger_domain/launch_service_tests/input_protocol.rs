use super::*;

#[tokio::test]
async fn launch_input_protocol_is_selected_once_before_any_worker_or_flag_change() {
    let f = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let context = super::initiation::context(&f, false);
    let legacy = f
        .service
        .create_or_resume_as(&context, &f.public.public_id, f.request("legacy-protocol"))
        .await
        .unwrap();
    assert!(legacy.launch.input_protocol.is_none());
    let enabled = f.service.with_common_first_messages(true);
    let replay = enabled
        .create_or_resume_as(&context, &f.public.public_id, f.request("legacy-protocol"))
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, legacy.launch);
    let common = enabled
        .create_or_resume_as(&context, &f.public.public_id, f.request("common-protocol"))
        .await
        .unwrap();
    common
        .launch
        .input_protocol
        .as_ref()
        .unwrap()
        .require_launch(&common.launch)
        .unwrap();
    let mut different_actor = super::initiation::context(&f, true);
    let replay = f
        .service
        .create_or_resume_as(
            &different_actor,
            &f.public.public_id,
            f.request("common-protocol"),
        )
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, common.launch);
    assert!(!serde_json::to_value(&common.launch)
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("input_protocol"));
    different_actor.actor = crate::resource_access::ResourceActor::Trigger {
        subject: f.caller.subject.clone(),
        launch_id: common.launch.id.clone(),
    };
    assert_eq!(
        enabled
            .create_or_resume_as(
                &different_actor,
                &f.public.public_id,
                f.request("forged-protocol"),
            )
            .await,
        Err(LaunchServiceError::Forbidden)
    );
    assert_eq!(f.stats().await.launches_total, 2);
}

#[tokio::test]
async fn common_protocol_requires_actual_initiation_and_rejects_transplant_or_schema_drift() {
    let f = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let enabled = f.service.with_common_first_messages(true);
    assert_eq!(
        enabled
            .create_or_resume(
                &f.caller,
                &f.public.public_id,
                f.request("unknown-initiator"),
            )
            .await,
        Err(LaunchServiceError::Forbidden)
    );
    let accepted = enabled
        .create_or_resume_as(
            &super::initiation::context(&f, true),
            &f.public.public_id,
            f.request("known-initiator"),
        )
        .await
        .unwrap()
        .launch;
    let protocol = accepted.input_protocol.as_ref().unwrap();
    let mut changed = accepted.clone();
    changed.initiation = None;
    assert!(protocol.require_launch(&changed).is_err());
    changed = accepted.clone();
    changed.id = LaunchId::generate();
    assert!(protocol.require_launch(&changed).is_err());
    changed = accepted.clone();
    changed.command_fingerprint.push('a');
    assert!(protocol.require_launch(&changed).is_err());
    let mut wire = serde_json::to_value(protocol).unwrap();
    wire["schema_version"] = serde_json::json!(2);
    let unsupported: LaunchInputProtocol = serde_json::from_value(wire.clone()).unwrap();
    assert!(unsupported.require_launch(&accepted).is_err());
    wire["untrusted"] = serde_json::json!(true);
    assert!(serde_json::from_value::<LaunchInputProtocol>(wire).is_err());
    assert_eq!(f.stats().await.launches_total, 1);
}
