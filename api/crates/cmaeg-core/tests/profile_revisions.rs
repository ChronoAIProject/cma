use cmaeg_core::channel::{
    ChannelDefinition, ProfilePublication, ProfileRevisions, RetainedProfileRevision, Secret,
};
use cmaeg_core::ids::BindingId;

#[test]
fn sealed_revision_history_retains_original_provider_and_credential() {
    let mut channel: ChannelDefinition =
        serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    channel.profile_digest = channel.profile.digest();
    channel
        .secrets
        .hmac_secrets
        .push(("key-1".into(), Secret::new("synthetic-hmac")));
    channel.validate().unwrap();
    let original = channel.clone();
    let old_key = Secret::new("synthetic-retained-key");
    channel.secrets.provider_credential = Some(Secret::new("synthetic-current-key"));
    channel.provider.binding_id = BindingId::new("synthetic-successor-binding").unwrap();
    channel.profile.context.system_prompt = "new synthetic prompt".into();
    channel.profile_digest = channel.profile.digest();
    channel.secrets.profile_revisions = Some(ProfileRevisions {
        current_revision: 2,
        baseline_complete: true,
        last_publication: Some(ProfilePublication {
            expected_version: 1,
            request_fingerprint: format!("sha256:{}", "1".repeat(64)),
        }),
        retained: vec![RetainedProfileRevision {
            revision: 1,
            profile_digest: original.profile_digest.clone(),
            profile: serde_json::to_value(&original.profile).unwrap(),
            provider: original.provider.clone(),
            credential: old_key.clone(),
        }],
    });
    channel.validate().unwrap();
    let public = channel.to_public_json().to_string();
    assert!(!public.contains(old_key.as_str()));
    assert!(!public.contains("profile_revisions"));
    let retained: ChannelDefinition = serde_json::from_value(channel.to_blob_json()).unwrap();
    retained.validate().unwrap();
    let assigned = retained.profile_revision(1).unwrap();
    assert_eq!(assigned.provider, &original.provider);
    assert_eq!(assigned.profile, original.profile);
    assert_eq!(assigned.credential, &old_key);
    assert_eq!(
        retained.profile_revision(2).unwrap().provider,
        &channel.provider
    );
    assert!(retained.profile_revision(3).is_none());
    let mut invalid = retained;
    invalid.secrets.profile_revisions.as_mut().unwrap().retained[0].profile_digest =
        channel.profile_digest;
    assert!(invalid.validate().is_err());
}
