use super::{fixture::Fixture, home_tests, model, runtime_tests, tests, NativePoolPodPolicy};
use crate::agent_sandbox::NativeTemplateNetworkPolicy;
use serde_json::json;

pub(super) fn policy(cidr: &str) -> NativeTemplateNetworkPolicy {
    NativeTemplateNetworkPolicy::parse(&json!({
        "ingress":[{"from":[{"ipBlock":{"cidr":cidr}}],"ports":[{"protocol":"TCP","port":9090}]}],
        "egress":[{"to":[{"ipBlock":{"cidr":"0.0.0.0/0","except":["10.0.0.0/8","172.16.0.0/12","192.168.0.0/16"]}}]}]
    }).to_string()).unwrap()
}

#[test]
fn old_policy_keeps_serialization_and_digest_without_inventing_network_defaults() {
    let old = r#"{"runtime_class":"gvisor","service_account":"runtime","image_pull_secrets":[]}"#;
    let policy: NativePoolPodPolicy = serde_json::from_str(old).unwrap();
    assert!(policy.template_network_policy.is_none());
    assert_eq!(serde_json::to_string(&policy).unwrap(), old);
    let mut config = super::super::tests::config();
    config.service_account = policy.service_account.clone();
    config.image_pull_secrets = policy.image_pull_secrets.clone();
    let spec = tests::spec();
    let expected = crate::provider_request_digest(&(
        "native-pool-v1",
        &config.namespace,
        &config.deployment,
        &config.runtime_class,
        &config.service_account,
        &config.image_pull_secrets,
        &spec,
    ))
    .unwrap();
    assert_eq!(model::digest(&config, &spec).unwrap(), expected);
    let template = model::template(&config, &spec).unwrap();
    assert!(template.data["spec"].get("networkPolicy").is_none());
    assert!(template.data["spec"]
        .get("networkPolicyManagement")
        .is_none());
}

#[test]
fn parser_requires_both_directions_and_refuses_unknown_unbounded_or_invalid_rules() {
    for value in [
        json!({"ingress":[]}),
        json!({"egress":[]}),
        json!({"ingress":[],"egress":[],"podSelector":{}}),
        json!({"ingress":[{"unknown":true}],"egress":[]}),
        json!({"ingress":[{"from":[{"ipBlock":{"cidr":"10.1.0.1/16"}}]}],"egress":[]}),
        json!({"ingress":[{"from":[{"ipBlock":{"cidr":"10.1.0.0/16","except":["192.168.0.0/16"]}}]}],"egress":[]}),
        json!({"ingress":[{"from":[{"ipBlock":{"cidr":"10.1.0.0/16"},"podSelector":{}}]}],"egress":[]}),
        json!({"ingress":[{"ports":[{"port":0}]}],"egress":[]}),
        json!({"ingress":[{"ports":[{"port":9090,"endPort":8080}]}],"egress":[]}),
        json!({"ingress":[{"ports":[{"port":"control"}]}],"egress":[]}),
        json!({"ingress":vec![json!({});65],"egress":[]}),
    ] {
        assert!(
            NativeTemplateNetworkPolicy::parse(&value.to_string()).is_err(),
            "{value}"
        );
    }
    assert!(NativeTemplateNetworkPolicy::parse(&" ".repeat(65537)).is_err());
    assert!(NativeTemplateNetworkPolicy::parse(r#"{"ingress":[],"egress":[]}"#).is_ok());
    let parsed = policy("10.254.0.0/16");
    assert_eq!(
        serde_json::from_str::<NativeTemplateNetworkPolicy>(
            &serde_json::to_string(&parsed).unwrap()
        )
        .unwrap(),
        parsed
    );
}

#[tokio::test]
async fn original_explicit_policy_survives_lost_ack_restart_inventory_and_resize() {
    let mut fixture = Fixture::new().await;
    fixture.client.config.template_network_policy = Some(policy("10.254.0.0/16"));
    let plan = fixture.client.plan(&tests::spec()).unwrap();
    fixture.lost("POST", "sandboxtemplates");
    fixture.lost("POST", "sandboxwarmpools");
    let original = fixture.client.configure_planned(&plan, 0).await.unwrap();
    let template = fixture.row("sandboxtemplates", &original.binding.template_name);
    assert_eq!(template["spec"]["networkPolicyManagement"], "Managed");
    assert_eq!(
        template["spec"]["networkPolicy"],
        serde_json::to_value(policy("10.254.0.0/16")).unwrap()
    );
    fixture.client.config.template_network_policy = Some(policy("10.42.0.0/16"));
    assert_ne!(
        fixture.client.plan(&tests::spec()).unwrap().config_sha256,
        plan.config_sha256
    );
    assert_eq!(
        fixture.client.configure_planned(&plan, 0).await.unwrap(),
        original
    );
    assert_eq!(
        fixture.client.inventory().await.unwrap(),
        vec![original.clone()]
    );
    assert_eq!(
        fixture
            .client
            .resize(&original.binding, 1)
            .await
            .unwrap()
            .binding,
        original.binding
    );
    let mut tampered = plan;
    tampered.pod_policy.template_network_policy =
        fixture.client.config.template_network_policy.clone();
    assert!(fixture
        .client
        .configure_planned(&tampered, 0)
        .await
        .is_err());
}

#[tokio::test]
async fn explicit_template_policy_drift_refuses_before_any_write() {
    for field in ["egress", "management", "extra_peer_field"] {
        let mut fixture = Fixture::new().await;
        fixture.client.config.template_network_policy = Some(policy("10.254.0.0/16"));
        let binding = fixture
            .client
            .configure(&tests::spec(), 0)
            .await
            .unwrap()
            .binding;
        fixture.edit(
            "sandboxtemplates",
            &binding.template_name,
            |v| match field {
                "egress" => v["spec"]["networkPolicy"]["egress"] = json!([{}]),
                "management" => v["spec"]["networkPolicyManagement"] = json!("Unmanaged"),
                _ => v["spec"]["networkPolicy"]["ingress"][0]["from"][0]["podSelector"] = json!({}),
            },
        );
        let before = fixture.state.lock().unwrap().calls.len();
        assert!(fixture.client.resize(&binding, 1).await.is_err(), "{field}");
        assert!(fixture.client.inventory().await.is_err(), "{field}");
        assert!(fixture.state.lock().unwrap().calls[before..]
            .iter()
            .all(|(verb, _)| verb.starts_with("GET ")));
    }
}

#[tokio::test]
async fn assigned_core_sandbox_projects_only_core_fields_and_retains_policy_selector_through_resume(
) {
    let mut fixture = Fixture::new().await;
    fixture.client.config.template_network_policy = Some(policy("10.254.0.0/16"));
    let mut spec = tests::spec();
    spec.home.mode = crate::HomeVolumeMode::NativeVctEmptyV1;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    let claim = home_tests::assigned_spec(&fixture, &spec).await;
    let home = home_tests::adopted(&fixture, &claim).await;
    let original = fixture.state.lock().unwrap().rows[home_tests::MEMBER]["spec"].clone();
    for field in [
        "networkPolicy",
        "networkPolicyManagement",
        "volumeClaimTemplatesPolicy",
    ] {
        assert!(original.get(field).is_none(), "{field}");
    }
    let selector = "agents.x-k8s.io/sandbox-template-ref-hash";
    let expected = super::super::pods::name_hash(&claim.request.pool.template_name);
    assert_eq!(
        original["podTemplate"]["metadata"]["labels"][selector],
        expected
    );
    fixture.client.config.template_network_policy = Some(policy("10.42.0.0/16"));
    let client = runtime_tests::client(&fixture, true);
    let previous = runtime_tests::request(&home, 1);
    let runtime = client
        .admit_pool_runtime(&claim, &home, &previous)
        .await
        .unwrap();
    client.suspend(&runtime, &previous).await.unwrap();
    client
        .resume(&runtime, &previous, &runtime_tests::request(&home, 2))
        .await
        .unwrap();
    let state = fixture.state.lock().unwrap();
    let after = &state.rows[home_tests::MEMBER]["spec"];
    assert_eq!(after["podTemplate"], original["podTemplate"]);
    assert_eq!(
        after["podTemplate"]["metadata"]["labels"][selector],
        expected
    );
    for (_, body) in &state.calls {
        if body["kind"] == "Sandbox"
            || body.get("spec").is_some() && body["metadata"]["uid"] == "member-uid"
        {
            assert!(body["spec"].get("networkPolicy").is_none());
            assert!(body["spec"].get("networkPolicyManagement").is_none());
        }
    }
    assert!(!state
        .calls
        .iter()
        .any(|(path, _)| path.contains("networkpolicies")));
}

#[tokio::test]
async fn changed_or_missing_template_selector_refuses_home_adoption_before_writes() {
    for replacement in [json!("other-hash"), serde_json::Value::Null] {
        let mut fixture = Fixture::new().await;
        fixture.client.config.template_network_policy = Some(policy("10.254.0.0/16"));
        let claim = home_tests::assigned(&fixture).await;
        let before = {
            let mut state = fixture.state.lock().unwrap();
            state.rows.get_mut(home_tests::MEMBER).unwrap()["spec"]["podTemplate"]["metadata"]
                ["labels"]["agents.x-k8s.io/sandbox-template-ref-hash"] = replacement;
            state.calls.len()
        };
        assert!(fixture.client.adopt_home(&claim).await.is_err());
        assert!(fixture.state.lock().unwrap().calls[before..]
            .iter()
            .all(|(verb, _)| verb.starts_with("GET ")));
    }
}

#[tokio::test]
async fn changed_template_refuses_adoption_assignment_and_resume_before_any_write_or_key_delivery()
{
    for phase in ["adopt", "assign", "resume"] {
        let mut fixture = Fixture::new().await;
        fixture.client.config.template_network_policy = Some(policy("10.254.0.0/16"));
        let claim = home_tests::assigned(&fixture).await;
        let (client, home, runtime, previous) = if phase == "adopt" {
            (None, None, None, None)
        } else {
            let home = home_tests::adopted(&fixture, &claim).await;
            let client = runtime_tests::client(&fixture, false);
            let previous = runtime_tests::request(&home, 1);
            let runtime = client
                .admit_pool_runtime(&claim, &home, &previous)
                .await
                .unwrap();
            if phase == "resume" {
                client.suspend(&runtime, &previous).await.unwrap();
            }
            (Some(client), Some(home), Some(runtime), Some(previous))
        };
        fixture.edit("sandboxtemplates", &claim.request.pool.template_name, |v| {
            v["spec"]["networkPolicy"]["egress"] = json!([{}]);
        });
        let before = fixture.state.lock().unwrap().calls.len();
        match phase {
            "adopt" => assert!(fixture.client.adopt_home(&claim).await.is_err()),
            "assign" => {
                let pod = crate::agent_sandbox::PodBinding {
                    sandbox_name: "warm-member".into(),
                    sandbox_uid: "member-uid".into(),
                    pod_name: "waiting-pod".into(),
                    pod_uid: "pod-uid".into(),
                    pod_ip: "10.0.0.1".parse().unwrap(),
                };
                assert!(client
                    .as_ref()
                    .unwrap()
                    .assign_pool_runtime(
                        runtime.as_ref().unwrap(),
                        &pod,
                        previous.as_ref().unwrap()
                    )
                    .await
                    .is_err());
            }
            _ => assert!(client
                .as_ref()
                .unwrap()
                .resume(
                    runtime.as_ref().unwrap(),
                    previous.as_ref().unwrap(),
                    &runtime_tests::request(home.as_ref().unwrap(), 2)
                )
                .await
                .is_err()),
        }
        let state = fixture.state.lock().unwrap();
        assert!(
            state.calls[before..]
                .iter()
                .all(|(call, _)| call.starts_with("GET ")),
            "{phase}"
        );
        assert!(state.assignments.is_empty(), "{phase}");
    }
}
