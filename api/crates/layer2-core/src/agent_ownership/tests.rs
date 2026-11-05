use super::*;

fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 30_000,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 256,
    }
}

fn membership(ids: &[&str]) -> Membership {
    Membership {
        now_ms: 1,
        members: ids.iter().map(|id| member(id)).collect(),
    }
}

fn winner(agent: &AgentId, membership: &Membership) -> String {
    candidates(agent, membership, None).unwrap()[0]
        .instance_id
        .clone()
}

#[test]
fn placement_is_order_independent_for_two_three_and_five_instances() {
    for ids in [
        vec!["a", "b"],
        vec!["a", "b", "c"],
        vec!["a", "b", "c", "d", "e"],
    ] {
        let before = membership(&ids);
        let mut reordered = before.clone();
        reordered.members.reverse();
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for index in 0..2_000 {
            let agent = AgentId(format!("agent-{index}"));
            let selected = winner(&agent, &before);
            assert_eq!(selected, winner(&agent, &reordered));
            *counts.entry(selected).or_default() += 1;
        }
        assert_eq!(counts.len(), ids.len());
        for count in counts.values() {
            // Guard against accidentally pinning nearly all work to one member.
            assert!(*count > 1_000 / ids.len());
            assert!(*count < 3_000 / ids.len());
        }
    }
}

#[test]
fn scale_up_moves_only_to_new_member_and_scale_down_only_from_departed_member() {
    let two = membership(&["a", "b"]);
    let three = membership(&["a", "b", "c"]);
    let mut moved = 0;
    for index in 0..1_000 {
        let agent = AgentId(format!("agent-{index}"));
        let before = winner(&agent, &two);
        let after = winner(&agent, &three);
        if before != after {
            assert_eq!(after, "c");
            moved += 1;
        }
        if after != "c" {
            assert_eq!(winner(&agent, &two), after);
        }
    }
    assert!((250..450).contains(&moved));
}

#[test]
fn expired_draining_and_full_members_cannot_receive_new_claims() {
    let mut members = membership(&["expired", "draining", "full", "available"]);
    members.members[0].expires_at_ms = members.now_ms;
    members.members[1].draining = true;
    members.members[2].owned_agents = 256;
    let agent = AgentId("agent".into());
    let available = candidates(&agent, &members, None).unwrap();
    assert_eq!(available.len(), 1);
    assert_eq!(available[0].instance_id, "available");
    let with_current = candidates(&agent, &members, Some("full")).unwrap();
    assert_eq!(with_current.len(), 2);
    members.members[3].owned_agents = 256;
    assert!(candidates(&agent, &members, None).unwrap().is_empty());
}

#[test]
fn incompatible_live_protocol_blocks_the_entire_membership_until_expiry() {
    let mut members = membership(&["old", "new"]);
    members.members[1].protocol += 1;
    members.members[1].draining = true;
    assert_eq!(
        candidates(&AgentId("agent".into()), &members, None),
        Err(OwnershipError::IncompatibleProtocol)
    );
    members.members[1].expires_at_ms = members.now_ms;
    assert_eq!(winner(&AgentId("agent".into()), &members), "old");
}
