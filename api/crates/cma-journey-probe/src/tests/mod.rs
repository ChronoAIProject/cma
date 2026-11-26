mod ledger;
mod monitor;
mod wire;

use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    ids::{ChannelId, RecordId},
    seal::{ChannelPointer, MasterKeyRing},
};
use serde_json::{json, Value};
use url::Url;

use crate::{
    config::Config,
    model::{Check, Job, Mode},
};

fn targets(base: &Url, fixture: &Path, suffix: &str) -> Value {
    json!({
        "agui_agent": format!("agt_agui_{suffix}"), "responses_agent": format!("agt_responses_{suffix}"),
        "gateway": {"channel": channel(suffix), "consumer":format!("consumer_{suffix}"),
            "event_type":"journey.test", "source":base.join("/source").unwrap(),
            "ingress":base.join(&format!("/custom/{suffix}")).unwrap(), "secret_file":fixture.join("secret")}
    })
}

pub(super) fn config(base: &Url, fixture: &Path) -> Arc<Config> {
    let path = fixture.join("config.json");
    std::fs::write(&path, config_value(base, fixture).to_string()).unwrap();
    Arc::new(Config::load(&path).unwrap())
}

fn config_value(base: &Url, fixture: &Path) -> Value {
    for name in ["owner", "gateway", "secret", "cookie"] {
        std::fs::write(fixture.join(name), format!("synthetic-{name}-value\n")).unwrap();
    }
    json!({
        "environment":"local", "location":"test", "revision":"rev-1", "candidate":"sha256:local",
        "runner_sha":option_env!("CMA_JOURNEY_BUILD_SHA").unwrap_or("0123456789012345678901234567890123456789"),
        "state_path":fixture.join("state"), "witness_path":fixture.join("witness"), "listen":"127.0.0.1:0",
        "web":base, "issuer":base, "cma":base, "gateway":base, "owner_bearer_file":fixture.join("owner"),
        "gateway_bearer_file":fixture.join("gateway"), "browser":{"cookie_file":fixture.join("cookie"),"subject":"subject_test"},
        "reference":{"agent":"agt_read","sandbox":"sbx_read","run":"3","after":2,"answer":"reference-answer",
            "channel":channel("read"),"event":"event_read","consumer":"consumer_read"},
        "deterministic_enabled":true, "deterministic":targets(base,fixture,"det"),
        "funded":{"targets":targets(base,fixture,"paid"),"prompt":"Reply with a short greeting",
            "budget":{"max_attempt_cost":1,"hourly_cost":1,"daily_cost":1,"hourly_attempts":1,"daily_attempts":1}}
    })
}

fn gateway_config_value(base: &Url, fixture: &Path) -> Value {
    let mut value = config_value(base, fixture);
    value["gateway"] = json!(base.join("gateway/").unwrap());
    value["deterministic"]["gateway"]["ingress"] = json!(base.join("gateway/custom/det").unwrap());
    value["funded"]["targets"]["gateway"]["ingress"] =
        json!(base.join("gateway/custom/paid").unwrap());
    value
}

pub(super) fn job(config: &Config, check: Check, mode: Mode) -> Job {
    config
        .jobs()
        .into_iter()
        .map(|(job, _)| job)
        .find(|job| job.check == check && job.mode == mode)
        .unwrap()
}

fn channel(kind: &str) -> &'static str {
    static CHANNELS: OnceLock<[ChannelId; 3]> = OnceLock::new();
    let sealed = CHANNELS.get_or_init(|| {
        ["read", "det", "paid"].map(|name| {
            let mut key = [0; 32];
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut key).unwrap();
            let ring = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode(key))).unwrap();
            let record_id = RecordId::new(format!("synthetic_{name}_record")).unwrap();
            let record_url =
                format!("https://gateway.example.test/api/v1/gateway/channel-records/{record_id}");
            let id = ChannelPointer {
                v: 1,
                record_url,
                record_id,
            }
            .seal_id(&ring)
            .unwrap();
            assert!(id.as_str().len() > 128 && id.as_str().len() <= 512);
            id
        })
    });
    sealed[match kind {
        "read" => 0,
        "det" => 1,
        "paid" => 2,
        _ => panic!("unknown fixture channel"),
    }]
    .as_str()
}
