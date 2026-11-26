use super::*;

#[test]
fn missing_or_mismatched_inventory_is_unknown_and_keeps_last_ready_count() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        publish(
            "agent",
            Some(Capacity {
                desired: 3,
                ready: 2,
                complete: true,
            }),
            10.,
        );
        let target = super::super::tests::target();
        let missing = capacity(&[&target], &[])[0];
        assert!(!missing.complete);
        publish("agent", Some(missing), 20.);
        publish("agent", None, 30.);
    });
    let text = handle.render();
    assert!(text.contains("cma_warm_pool_observation_complete{kind=\"agent\"} 0"));
    assert!(text.contains("cma_warm_pool_ready_replicas{kind=\"agent\"} 2"));
    assert!(text.contains("cma_warm_pool_sample_success_timestamp_seconds{kind=\"agent\"} 10"));
}

#[test]
fn capacity_separates_machine_and_agent_and_requires_exact_observed_binding() {
    let mut agent = super::super::tests::target();
    let binding = layer3_adapter::agent_sandbox::NativePoolBinding {
        name: "pool".into(),
        uid: "pool-uid".into(),
        template_name: "template".into(),
        template_uid: "template-uid".into(),
        spec: agent.plan.spec.clone(),
        pod_policy: agent.plan.pod_policy.clone(),
        config_sha256: agent.plan.config_sha256.clone(),
    };
    agent.binding = Some(binding.clone());
    agent.applied_revision = agent.revision;
    let mut machine = agent.clone();
    machine.machine = true;
    machine.target_replicas = 0;
    machine.binding = None;
    let status = NativePoolStatus {
        binding,
        desired_replicas: 3,
        ready_replicas: 2,
        observed: true,
    };
    let counts = capacity(&[&agent, &machine], std::slice::from_ref(&status));
    assert_eq!(
        (counts[0].desired, counts[0].ready, counts[0].complete),
        (3, 2, true)
    );
    assert_eq!(
        (counts[1].desired, counts[1].ready, counts[1].complete),
        (0, 0, true)
    );
    let mut wrong = status.clone();
    wrong.binding.uid = "replacement".into();
    assert!(!capacity(&[&agent], &[wrong])[0].complete);
    let mut unobserved = status;
    unobserved.observed = false;
    assert!(!capacity(&[&agent], &[unobserved])[0].complete);
}

#[test]
fn empty_capacity_recovers_only_when_the_same_target_is_full() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut target = super::super::tests::target();
    let binding = NativePoolBinding {
        name: "pool".into(),
        uid: "pool-uid".into(),
        template_name: "template".into(),
        template_uid: "template-uid".into(),
        spec: target.plan.spec.clone(),
        pod_policy: target.plan.pod_policy.clone(),
        config_sha256: target.plan.config_sha256.clone(),
    };
    target.binding = Some(binding.clone());
    target.target_replicas = 2;
    target.applied_revision = target.revision;
    let mut status = NativePoolStatus {
        binding,
        desired_replicas: 2,
        ready_replicas: 0,
        observed: true,
    };
    let mut previous = BTreeMap::new();
    metrics::with_local_recorder(&recorder, || {
        for (ready, at) in [(0, 10.), (1, 20.)] {
            status.ready_replicas = ready;
            publish(
                "agent",
                Some(capacity(&[&target], std::slice::from_ref(&status))[0]),
                at,
            );
            observe_capacity_changes(&[&target], std::slice::from_ref(&status), &mut previous);
            assert!(previous[&target.id].degraded);
            assert!(handle
                .render()
                .contains("cma_warm_pool_capacity_degraded{kind=\"agent\"} 1"));
        }
        publish("agent", Some(capacity(&[&target], &[])[0]), 30.);
        observe_capacity_changes(&[&target], &[], &mut previous);
        assert!(previous[&target.id].degraded);
        assert!(handle
            .render()
            .contains("cma_warm_pool_observation_complete{kind=\"agent\"} 0"));
        status.ready_replicas = 2;
        publish(
            "agent",
            Some(capacity(&[&target], std::slice::from_ref(&status))[0]),
            40.,
        );
        observe_capacity_changes(&[&target], &[status], &mut previous);
        assert!(!previous[&target.id].degraded);
        assert_eq!(previous[&target.id].binding, target.binding.unwrap());
        assert!(handle
            .render()
            .contains("cma_warm_pool_capacity_degraded{kind=\"agent\"} 0"));
    });
}

#[test]
fn only_new_successful_revision_is_progress() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let mut target = super::super::tests::target();
        applied(&target, false);
        assert!(!handle
            .render()
            .contains("cma_warm_pool_revision_applied_timestamp_seconds"));
        target.applied_revision = target.revision;
        applied(&target, true);
        assert!(!handle
            .render()
            .contains("cma_warm_pool_revision_applied_timestamp_seconds"));
        target.revision += 1;
        applied(&target, true);
        assert!(handle
            .render()
            .contains("cma_warm_pool_revision_applied_timestamp_seconds"));
    });
}
