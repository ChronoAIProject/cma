use super::*;
use std::collections::BTreeMap;

const PLATFORMS: [BridgePlatformFlag; 8] = [
    BridgePlatformFlag::Telegram,
    BridgePlatformFlag::TelegramNew,
    BridgePlatformFlag::Discord,
    BridgePlatformFlag::Lark,
    BridgePlatformFlag::Feishu,
    BridgePlatformFlag::Slack,
    BridgePlatformFlag::Whatsapp,
    BridgePlatformFlag::X,
];
const STAGES: [BridgeStage; 5] = [
    BridgeStage::Management,
    BridgeStage::Ingress,
    BridgeStage::Allocation,
    BridgeStage::Execution,
    BridgeStage::Outbound,
];

fn configured(flags: &[BridgeFlag]) -> BridgeFeatureFlags {
    BridgeFeatureFlags::from_lookup(&mut |key| {
        Ok(flags
            .iter()
            .any(|flag| flag.key() == key)
            .then(|| "true".into()))
    })
    .unwrap()
}

#[test]
fn existing_deployment_defaults_close_every_new_channel_effect() {
    let mut seen = Vec::new();
    let flags = BridgeFeatureFlags::from_lookup(&mut |key| {
        seen.push(key.to_owned());
        Ok(None)
    })
    .unwrap();
    assert_eq!(flags, BridgeFeatureFlags::default());
    assert_eq!(seen.len(), BridgeFlag::ALL.len());
    for platform in PLATFORMS {
        for stage in STAGES {
            assert!(flags.check_new_effect(stage, platform).is_err());
        }
    }
    assert!(!flags.enabled(BridgeFlag::V2UiTransport));
    assert!(!flags.enabled(BridgeFlag::TriggerCommandAdmission));
    assert!(!flags.enabled(BridgeFlag::ControlAdmission));
}

#[test]
fn all_flag_configurations_keep_each_flag_independent() {
    for bits in 0..(1 << BridgeFlag::ALL.len()) {
        let selected: Vec<_> = BridgeFlag::ALL
            .into_iter()
            .filter(|flag| bits & (1 << (*flag as usize)) != 0)
            .collect();
        let flags = configured(&selected);
        for flag in BridgeFlag::ALL {
            assert_eq!(flags.enabled(flag), selected.contains(&flag));
        }
        for platform in PLATFORMS {
            for stage in STAGES {
                assert_eq!(
                    flags.check_new_effect(stage, platform).is_ok(),
                    selected.contains(&stage.flag()) && selected.contains(&platform.flag()),
                );
            }
        }
        assert_eq!(flags.check_command_release(None), Ok(()));
    }
}

#[test]
fn invalid_boolean_never_enables_a_flag_or_echoes_its_value() {
    for flag in BridgeFlag::ALL {
        for value in [
            "",
            "1",
            "0",
            "TRUE",
            "False",
            " true",
            "false ",
            "private-value",
        ] {
            let error = BridgeFeatureFlags::from_lookup(&mut |key| {
                Ok((key == flag.key()).then(|| value.to_owned()))
            })
            .unwrap_err();
            assert_eq!(error, format!("{} must be true or false", flag.key()));
        }
    }
}

#[test]
fn lookup_failure_is_not_interpreted_as_disabled_or_enabled() {
    assert_eq!(
        BridgeFeatureFlags::from_lookup(&mut |_| Err("lookup unavailable".into())),
        Err("lookup unavailable".into()),
    );
}

#[test]
fn explicit_false_matches_an_absent_setting() {
    assert_eq!(
        BridgeFeatureFlags::from_lookup(&mut |_| Ok(Some("false".into()))).unwrap(),
        BridgeFeatureFlags::default(),
    );
    let keys: BTreeMap<_, _> = BridgeFlag::ALL.into_iter().map(|f| (f.key(), f)).collect();
    assert_eq!(keys.len(), BridgeFlag::ALL.len());
}

#[test]
fn ingress_can_stop_while_durable_allocation_execution_and_outbound_drain() {
    let flags = configured(&[
        BridgeFlag::Allocation,
        BridgeFlag::Execution,
        BridgeFlag::Outbound,
        BridgeFlag::Telegram,
    ]);
    assert_eq!(
        flags.check_new_effect(BridgeStage::Ingress, BridgePlatformFlag::Telegram),
        Err(BridgeFlag::Ingress),
    );
    for stage in [
        BridgeStage::Allocation,
        BridgeStage::Execution,
        BridgeStage::Outbound,
    ] {
        assert_eq!(
            flags.accepted_work(
                stage,
                BridgePlatformFlag::Telegram,
                AcceptedEffectState::ReadyBeforeEffect
            ),
            AcceptedWorkDecision::CheckAuthority,
        );
    }
}

#[test]
fn outbound_disable_does_not_pause_execution_or_an_unrelated_platform() {
    let flags = configured(&[BridgeFlag::Execution, BridgeFlag::Telegram]);
    assert_eq!(
        flags.check_command_release(Some(BridgePlatformFlag::Telegram)),
        Ok(())
    );
    assert_eq!(
        flags.check_new_effect(BridgeStage::Outbound, BridgePlatformFlag::Telegram),
        Err(BridgeFlag::Outbound),
    );
    assert_eq!(
        flags.check_command_release(Some(BridgePlatformFlag::Discord)),
        Err(BridgeFlag::Discord),
    );
}

#[test]
fn unknown_started_terminal_and_expired_work_never_become_a_new_effect_on_reenable() {
    for flags in [BridgeFeatureFlags::default(), configured(&BridgeFlag::ALL)] {
        for stage in STAGES {
            for platform in PLATFORMS {
                for (state, decision) in [
                    (
                        AcceptedEffectState::EffectStarted,
                        AcceptedWorkDecision::ReconcileExisting,
                    ),
                    (
                        AcceptedEffectState::OutcomeUnknown,
                        AcceptedWorkDecision::RequireExplicitRecovery,
                    ),
                    (
                        AcceptedEffectState::Terminal,
                        AcceptedWorkDecision::ObserveTerminal,
                    ),
                    (
                        AcceptedEffectState::ExpiredBeforeEffect,
                        AcceptedWorkDecision::RecordExpiry,
                    ),
                ] {
                    assert_eq!(flags.accepted_work(stage, platform, state), decision);
                }
            }
        }
    }
}

#[test]
fn reenabling_unstarted_work_still_requires_current_authority() {
    let before = BridgeFeatureFlags::default();
    let after = configured(&[BridgeFlag::Allocation, BridgeFlag::Feishu]);
    let work = AcceptedEffectState::ReadyBeforeEffect;
    assert_eq!(
        before.accepted_work(BridgeStage::Allocation, BridgePlatformFlag::Feishu, work),
        AcceptedWorkDecision::Hold(BridgeFlag::Allocation),
    );
    assert_eq!(
        after.accepted_work(BridgeStage::Allocation, BridgePlatformFlag::Feishu, work),
        AcceptedWorkDecision::CheckAuthority,
    );
}

#[test]
fn execution_disable_drains_released_fifo_without_releasing_held_commands() {
    let flags = BridgeFeatureFlags::default();
    for platform in PLATFORMS {
        assert_eq!(
            flags.channel_command_work(platform, ChannelCommandGateState::HeldBeforeRelease),
            AcceptedWorkDecision::Hold(BridgeFlag::Execution),
        );
        assert_eq!(
            flags.channel_command_work(platform, ChannelCommandGateState::ReleasedIntoSharedFifo),
            AcceptedWorkDecision::CheckAuthority,
        );
    }
}

#[test]
fn drain_policy_never_replays_bound_terminal_or_expired_commands() {
    for flags in [BridgeFeatureFlags::default(), configured(&BridgeFlag::ALL)] {
        for platform in PLATFORMS {
            for (state, decision) in [
                (
                    ChannelCommandGateState::DispatchBound,
                    AcceptedWorkDecision::ReconcileExisting,
                ),
                (
                    ChannelCommandGateState::Terminal,
                    AcceptedWorkDecision::ObserveTerminal,
                ),
                (
                    ChannelCommandGateState::ExpiredBeforeDispatch,
                    AcceptedWorkDecision::RecordExpiry,
                ),
            ] {
                assert_eq!(flags.channel_command_work(platform, state), decision);
            }
        }
    }
}
