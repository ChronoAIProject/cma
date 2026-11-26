use url::Url;

use crate::AdapterError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NativeRuntimeIdentity {
    pub epoch: u64,
    pub sandbox_id: Option<String>,
    pub request_sha256: Option<String>,
}

const MAX_ENDPOINT_BYTES: usize = 2_048;

/// A provider proxy prefix resolved to the schemes trusted by configuration.
#[derive(Clone)]
pub(super) struct RuntimeEndpoint {
    base: Url,
    secure: bool,
    control_token: Option<String>,
    accept: Option<&'static str>,
    native_identity: Option<NativeRuntimeIdentity>,
    native_binding: Option<(crate::RuntimeHandle, crate::agent_sandbox::PodBinding)>,
    direct_receiver: Option<(crate::RuntimeHandle, crate::RetirementPod)>,
}

impl RuntimeEndpoint {
    pub(super) fn parse(provider_base: &str, advertised: &str) -> crate::Result<Self> {
        let configured = Url::parse(provider_base)
            .map_err(|_| rejected("the provider base URL is not absolute"))?;
        let (http_scheme, secure) = match configured.scheme() {
            "http" => ("http", false),
            "https" => ("https", true),
            _ => return Err(rejected("the provider base URL scheme is not supported")),
        };
        if advertised.is_empty()
            || advertised.len() > MAX_ENDPOINT_BYTES
            || advertised
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
            || advertised.starts_with("//")
        {
            return Err(rejected(
                "the provider endpoint is not a valid proxy address",
            ));
        }

        let explicit_scheme = advertised.split_once("://").map(|(scheme, _)| scheme);
        let mut parsed = match explicit_scheme {
            Some(scheme)
                if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") =>
            {
                Url::parse(advertised)
                    .map_err(|_| rejected("the provider endpoint is malformed"))?
            }
            Some(_) => {
                return Err(rejected("the provider endpoint scheme is not supported"));
            }
            None => {
                if advertised
                    .get(..5)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http:"))
                    || advertised
                        .get(..6)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https:"))
                {
                    return Err(rejected("the provider endpoint scheme is not supported"));
                }
                Url::parse(&format!("{http_scheme}://{advertised}"))
                    .map_err(|_| rejected("the provider endpoint is malformed"))?
            }
        };
        if parsed.host().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.cannot_be_a_base()
        {
            return Err(rejected(
                "the provider endpoint contains an invalid authority or suffix",
            ));
        }
        parsed
            .set_scheme(http_scheme)
            .map_err(|_| rejected("the provider endpoint scheme could not be normalized"))?;
        Ok(Self {
            base: parsed,
            secure,
            control_token: None,
            accept: None,
            native_binding: None,
            native_identity: None,
            direct_receiver: None,
        })
    }

    pub(super) fn with_control_token(mut self, token: String) -> Self {
        self.control_token = Some(token);
        self
    }

    pub(super) fn control_token(&self) -> Option<&str> {
        self.control_token.as_deref()
    }

    pub(super) fn with_accept(mut self, accept: &'static str) -> Self {
        self.accept = Some(accept);
        self
    }

    pub(super) fn accept(&self) -> Option<&'static str> {
        self.accept
    }

    pub(super) fn with_native_binding(
        mut self,
        handle: crate::RuntimeHandle,
        binding: crate::agent_sandbox::PodBinding,
    ) -> Self {
        self.native_binding = Some((handle, binding));
        self
    }

    pub(super) fn native_binding(
        &self,
    ) -> Option<&(crate::RuntimeHandle, crate::agent_sandbox::PodBinding)> {
        self.native_binding.as_ref()
    }

    pub(super) fn with_direct_receiver(
        mut self,
        handle: &crate::RuntimeHandle,
        binding: &crate::agent_sandbox::PodBinding,
    ) -> Self {
        self.direct_receiver = crate::RetirementPod::try_from(binding)
            .ok()
            .map(|pod| (handle.clone(), pod));
        self
    }

    pub(super) fn retirement_receiver(
        &self,
    ) -> Option<(crate::RuntimeHandle, crate::RetirementPod)> {
        self.native_binding
            .as_ref()
            .and_then(|(handle, binding)| {
                crate::RetirementPod::try_from(binding)
                    .ok()
                    .map(|pod| (handle.clone(), pod))
            })
            .or_else(|| self.direct_receiver.clone())
    }

    pub(super) fn with_native_identity(mut self, identity: NativeRuntimeIdentity) -> Self {
        self.native_identity = Some(identity);
        self
    }

    pub(super) fn native_identity(&self) -> Option<&NativeRuntimeIdentity> {
        self.native_identity.as_ref()
    }

    pub(super) fn control(&self) -> crate::Result<Url> {
        self.with_path(&["runtime", "control"], false)
    }

    pub(super) fn ownership(&self, agent: &str, action: &str) -> crate::Result<Url> {
        self.with_path(
            &["runtime", "agents", agent, "manager-ownership", action],
            false,
        )
    }

    pub(super) fn conversation(&self, agent_id: &str) -> crate::Result<Url> {
        self.with_path(&["runtime", "agents", agent_id, "conversation"], true)
    }

    pub(super) fn conversation_for_capacity_recovery(
        &self,
        agent_id: &str,
        challenge: &str,
    ) -> crate::Result<Url> {
        let mut url = self.conversation(agent_id)?;
        url.query_pairs_mut()
            .append_pair("capacityRecoveryChallenge", challenge);
        Ok(url)
    }

    pub(super) fn capacity_recovery(&self, agent_id: &str) -> crate::Result<Url> {
        self.with_path(&["runtime", "agents", agent_id, "capacity-recovery"], false)
    }

    pub(super) fn stale_turn(&self, agent_id: &str, action: &str) -> crate::Result<Url> {
        self.with_path(
            &["runtime", "agents", agent_id, "stale-turn", action],
            false,
        )
    }

    pub(super) fn history(&self, agent_id: &str) -> crate::Result<Url> {
        self.with_path(&["runtime", "agents", agent_id, "history"], false)
    }

    pub(super) fn desktop(&self) -> crate::Result<Url> {
        self.with_path(&["runtime", "desktop"], true)
    }

    pub(super) fn desktop_ownership(&self, action: &str) -> crate::Result<Url> {
        self.with_path(&["runtime", "desktop", "ownership", action], false)
    }

    pub(super) fn checkpoint(&self, reference: &str) -> crate::Result<Url> {
        self.with_path(&["runtime", "checkpoints", reference], false)
    }

    fn with_path(&self, suffix: &[&str], websocket: bool) -> crate::Result<Url> {
        let mut url = self.base.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| rejected("the provider endpoint cannot carry runtime paths"))?;
            segments.pop_if_empty();
            segments.extend(suffix.iter().copied());
        }
        if websocket {
            let scheme = if self.secure { "wss" } else { "ws" };
            url.set_scheme(scheme)
                .map_err(|_| rejected("the runtime WebSocket scheme could not be derived"))?;
        }
        Ok(url)
    }
}

fn rejected(detail: &str) -> AdapterError {
    AdapterError::Rejected(detail.into())
}

#[cfg(test)]
#[path = "provider_endpoint_tests.rs"]
mod tests;

impl std::fmt::Debug for RuntimeEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeEndpoint")
            .field("base", &self.base)
            .field("secure", &self.secure)
            .finish_non_exhaustive()
    }
}
