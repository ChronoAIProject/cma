use std::{collections::BTreeMap, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cma_service_auth::signing_keys::{self, Cache};
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const ISSUER: &str = "https://token.actions.githubusercontent.com";
const JWKS: &str = "https://token.actions.githubusercontent.com/.well-known/jwks";
const REPOSITORY: &str = "ChronoAIProject/cma";

pub(super) struct Identity {
    repository_id: String,
    keys: Cache<KeySet, ()>,
}

impl Identity {
    pub(super) fn new(repository_id: String) -> Result<Self, String> {
        if !decimal(&repository_id) {
            return Err("invalid runtime pointer proof repository identity".into());
        }
        let keys = Cache::new(
            signing_keys::Config::new(Duration::from_secs(300), 1),
            KeySet::parse,
            |_| (),
        )
        .map_err(|_| "runtime pointer proof signing-key client unavailable")?;
        Ok(Self {
            repository_id,
            keys,
        })
    }

    pub(super) async fn permits(
        &self,
        raw: &str,
        challenge: &str,
        environment: &str,
        now: u64,
    ) -> Option<()> {
        if raw.len() > 16_384 {
            return None;
        }
        let (signed, signature) = raw.rsplit_once('.')?;
        let (header, payload) = signed.split_once('.')?;
        let header: Header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
        if header.alg != "RS256" || !key_id(&header.kid) {
            return None;
        }
        let keys = self.keys.keys_for(JWKS, &header.kid).await.ok()?;
        let key = keys.0.get(&header.kid)?;
        let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
        RsaPublicKeyComponents {
            n: &key.n,
            e: &key.e,
        }
        .verify(&RSA_PKCS1_2048_8192_SHA256, signed.as_bytes(), &signature)
        .ok()?;
        let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        claims
            .matches(&self.repository_id, challenge, environment, now)
            .then_some(())
    }
}

fn decimal(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn key_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: String,
    repository: String,
    repository_id: String,
    environment: String,
    event_name: String,
    r#ref: String,
    ref_protected: String,
    workflow_ref: String,
    job_workflow_ref: String,
    iat: u64,
    nbf: u64,
    exp: u64,
}

impl Claims {
    fn matches(&self, repository_id: &str, challenge: &str, environment: &str, now: u64) -> bool {
        let (branch, deploy_job, source_workflows, deployment_environment): (_, _, &[_], _) =
            match environment {
                "staging" => (
                    "develop",
                    "staging-deploy.yml",
                    &["staging-cd.yml", "redeploy-staging.yml"],
                    "chrono-ai-staging",
                ),
                "production" => ("main", "prod-deploy.yml", &["prod-cd.yml"], "production"),
                _ => return false,
            };
        let workflow = |file| format!("{REPOSITORY}/.github/workflows/{file}@refs/heads/{branch}");
        self.iss == ISSUER
            && self.aud
                == format!(
                    "cma-runtime-proof-v1:{:x}",
                    Sha256::digest(challenge.as_bytes())
                )
            && self.repository == REPOSITORY
            && self.repository_id == repository_id
            && self.environment == deployment_environment
            && matches!(self.event_name.as_str(), "push" | "workflow_dispatch")
            && self.r#ref == format!("refs/heads/{branch}")
            && self.ref_protected == "true"
            && self.job_workflow_ref == workflow(deploy_job)
            && source_workflows
                .iter()
                .any(|file| self.workflow_ref == workflow(file))
            && self.iat != 0
            && self.iat <= now.saturating_add(5)
            && now.saturating_sub(self.iat) <= 60
            && self.nbf <= now.saturating_add(5)
            && self.nbf < self.exp
            && self.exp > now
            && self.exp > self.iat
            && self.exp.saturating_sub(self.iat) <= 600
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(challenge: &str) -> Claims {
        serde_json::from_value(serde_json::json!({
            "iss": ISSUER,
            "aud": format!("cma-runtime-proof-v1:{:x}", Sha256::digest(challenge.as_bytes())),
            "repository": REPOSITORY,
            "repository_id": "12345",
            "environment": "chrono-ai-staging",
            "event_name": "push",
            "ref": "refs/heads/develop",
            "ref_protected": "true",
            "workflow_ref": format!("{REPOSITORY}/.github/workflows/staging-cd.yml@refs/heads/develop"),
            "job_workflow_ref": format!("{REPOSITORY}/.github/workflows/staging-deploy.yml@refs/heads/develop"),
            "iat": 500,
            "nbf": 500,
            "exp": 800,
        })).unwrap()
    }

    #[test]
    fn permits_only_the_current_challenge_and_the_protected_deploy_job() {
        let good = claims("exact request");
        assert!(good.matches("12345", "exact request", "staging", 501));
        assert!(!good.matches("12345", "different request", "staging", 501));
        assert!(!good.matches("12345", "exact request", "production", 501));
        assert!(!good.matches("12346", "exact request", "staging", 501));
        assert!(!good.matches("12345", "exact request", "staging", 561));
        let other = Claims {
            job_workflow_ref: good.workflow_ref.clone(),
            ..good
        };
        assert!(!other.matches("12345", "exact request", "staging", 501));
    }
}

#[derive(Deserialize)]
struct Key {
    kty: String,
    kid: String,
    alg: String,
    #[serde(rename = "use")]
    purpose: String,
    n: String,
    e: String,
}

struct RsaKey {
    n: Vec<u8>,
    e: Vec<u8>,
}

struct KeySet(BTreeMap<String, RsaKey>);

impl signing_keys::KeySet for KeySet {
    fn contains(&self, kid: &str) -> bool {
        self.0.contains_key(kid)
    }
}

impl KeySet {
    fn parse(bytes: &[u8]) -> Result<Self, ()> {
        #[derive(Deserialize)]
        struct Set {
            keys: Vec<Key>,
        }
        let set: Set = serde_json::from_slice(bytes).map_err(|_| ())?;
        if set.keys.is_empty() || set.keys.len() > 16 {
            return Err(());
        }
        let mut keys = BTreeMap::new();
        for key in set.keys {
            if key.kty != "RSA" || key.alg != "RS256" || key.purpose != "sig" || !key_id(&key.kid) {
                return Err(());
            }
            let n = URL_SAFE_NO_PAD.decode(key.n).map_err(|_| ())?;
            let e = URL_SAFE_NO_PAD.decode(key.e).map_err(|_| ())?;
            if !(256..=1024).contains(&n.len())
                || !(1..=8).contains(&e.len())
                || n[0] == 0
                || e[0] == 0
                || keys.insert(key.kid, RsaKey { n, e }).is_some()
            {
                return Err(());
            }
        }
        Ok(Self(keys))
    }
}
