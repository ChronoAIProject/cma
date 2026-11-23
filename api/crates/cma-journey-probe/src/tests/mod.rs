mod ledger;
mod wire;

use std::{path::Path, sync::Arc};

use serde_json::{json, Value};
use url::Url;

use crate::{
    config::Config,
    model::{Check, Job, Mode},
};

fn targets(base: &Url, fixture: &Path, suffix: &str) -> Value {
    json!({
        "agui_agent": format!("agt_agui_{suffix}"), "responses_agent": format!("agt_responses_{suffix}"),
        "gateway": {"channel": format!("channel_{suffix}"), "consumer":format!("consumer_{suffix}"),
            "event_type":"journey.test", "source":base.join("/source").unwrap(),
            "ingress":base.join(&format!("/custom/{suffix}")).unwrap(), "secret_file":fixture.join("secret")}
    })
}

fn config(base: &Url, fixture: &Path) -> Arc<Config> {
    for name in ["owner", "gateway", "secret", "cookie"] {
        std::fs::write(fixture.join(name), format!("synthetic-{name}-value\n")).unwrap();
    }
    let value = json!({
        "environment":"local", "location":"test", "revision":"rev-1", "candidate":"sha256:local",
        "state_path":fixture.join("state"), "witness_path":fixture.join("witness"), "listen":"127.0.0.1:0",
        "web":base, "issuer":base, "cma":base, "gateway":base, "owner_bearer_file":fixture.join("owner"),
        "gateway_bearer_file":fixture.join("gateway"), "browser":{"cookie_file":fixture.join("cookie"),"subject":"subject_test"},
        "reference":{"agent":"agt_read","sandbox":"sbx_read","run":"3","after":2,"answer":"reference-answer",
            "channel":"channel_read","event":"event_read","consumer":"consumer_read"},
        "deterministic_enabled":true, "deterministic":targets(base,fixture,"det"),
        "funded":{"targets":targets(base,fixture,"paid"),"prompt":"Reply with a short greeting",
            "budget":{"max_attempt_cost":1,"hourly_cost":1,"daily_cost":1,"hourly_attempts":1,"daily_attempts":1}}
    });
    let path = fixture.join("config.json");
    std::fs::write(&path, value.to_string()).unwrap();
    Arc::new(Config::load(&path).unwrap())
}

fn job(config: &Config, check: Check, mode: Mode) -> Job {
    config
        .jobs()
        .into_iter()
        .map(|(job, _)| job)
        .find(|job| job.check == check && job.mode == mode)
        .unwrap()
}
