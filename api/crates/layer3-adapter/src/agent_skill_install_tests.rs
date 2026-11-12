use super::*;
fn skill(version: &str) -> AgentSkill {
    AgentSkill {
        id: "research".into(),
        name: Some("Research".into()),
        version: Some(version.into()),
        archive_sha256: "a".repeat(64),
        source_url: "https://skills.example/archive".into(),
    }
}
#[test]
fn upgrade_hash_pins_previous_target_receipt_and_sequence_but_not_download_locations() {
    let original = AgentSkillInstallRequest::new(AgentSlot::new("agent"), skill("1.0"));
    let receipt = AgentSkillInstallReceipt::from_request(&original);
    let request = AgentSkillInstallRequest::upgrade(
        original.agent_id.clone(),
        skill("1.1"),
        original.skill.clone(),
        Some(receipt),
        2,
    );
    assert_ne!(request.operation_id, original.operation_id);
    assert!(request.has_valid_sha256());
    let mut moved = request.clone();
    moved.skill.source_url = "https://new.example/new".into();
    moved.upgrade.as_mut().unwrap().previous.source_url = "https://new.example/old".into();
    assert_eq!(request.computed_sha256(), moved.computed_sha256());
    for case in 0..5 {
        let mut altered = request.clone();
        match case {
            0 => altered.skill.version = Some("1.2".into()),
            1 => altered.upgrade.as_mut().unwrap().previous.archive_sha256 = "b".repeat(64),
            2 => altered.upgrade.as_mut().unwrap().previous_receipt = None,
            3 => altered.upgrade.as_mut().unwrap().sequence += 1,
            _ => altered.skill.name = Some("Changed".into()),
        }
        assert!(!altered.has_valid_sha256());
    }
    let later = AgentSkillInstallRequest::upgrade(
        original.agent_id,
        request.skill.clone(),
        original.skill,
        request.upgrade.as_ref().unwrap().previous_receipt.clone(),
        3,
    );
    assert_ne!(request.operation_id, later.operation_id);
}
#[test]
fn retained_additive_wire_shape_and_hash_remain_unchanged() {
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Predecessor {
        operation_id: String,
        request_sha256: String,
        agent_id: AgentSlot,
        skill: AgentSkill,
    }
    let request = AgentSkillInstallRequest::new(AgentSlot::new("agent"), skill("1.0"));
    let bytes = serde_json::to_vec(&request).unwrap();
    let decoded: Predecessor = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&decoded).unwrap());
    let upgraded =
        AgentSkillInstallRequest::upgrade(request.agent_id, skill("1.1"), request.skill, None, 1);
    assert!(
        serde_json::from_slice::<Predecessor>(&serde_json::to_vec(&upgraded).unwrap()).is_err()
    );
}
