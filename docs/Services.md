# Services: what a session may reach, and how it reaches it

A session can act on its owner's behalf against platform services — open a pull request on GitHub, read from object storage, call the model endpoint. This page is the whole path, in order, with the file and symbol that implements each step.

It exists because the path spans seven files and nobody could see it. Work has already been proposed against this mechanism on the assumption it did not exist, when it had shipped some time earlier.

**Two words are used precisely throughout.** A session's **reach** is the list of services its credential may call. A session's **capabilities** are what it can do — hold a conversation, render a terminal, be interrupted. They are different facts about the same session and neither implies the other.

---

## 1. The path, in order

### 1. The catalogue answers as the caller

```
GET /api/v1/environment-profiles/services
```

The request, response shape, mandatory identifiers, and no-catalogue behavior
are documented in the REST interface's
[Services catalogue](./API_REST.md#services-catalogue).

`layer1-public/src/rest/environments.rs::services` → `EnvironmentProfileService::available_services` (`layer2-core/src/environment_service.rs`).

Answered **as the caller**, with their own delegated credential, because which services exist is a fact about their account and this service's own identity would answer with a different list. The mandated entries travel alongside so a client can draw them as fixed rather than inferring which cannot be unticked.

A deployment with no catalogue configured answers `ServiceAvailability::NoCatalogue` (`layer2-core/src/environment_catalog.rs`) rather than an error. Nothing about that stops a session starting — see §4.

That no-catalogue case is distinct from an outage of a configured catalogue.
The direct picker route returns `503 service_catalogue_unavailable` when it
cannot ask for the caller's services; this route-specific response is
documented with the [Services catalogue](./API_REST.md#services-catalogue).

### 2. A profile records a selection

`EnvironmentProfileSpec::services` and `EnvironmentProfile::services` (`layer2-core/src/environment.rs`). The wire shape is the
[`nyxid.services` profile field](./API_REST.md#write-shape-profilerequest).
It is authored in `web/src/environment/ServicePicker.tsx`, either in the profile
editor or inline in the new-session dialog (`web/src/sessions/NewSessionDialog.tsx`),
which saves a real profile rather than a one-off selection.

A profile is a **named, reusable, auditable record**. That is why the selection lives there rather than on the session request: two sessions started from one profile must be the same session twice.

### 3. The session records what it was granted

`Session::granted_services` (`layer2-core/src/session.rs`), written when the session is created and read back by `SessionService::services_for` (`layer2-core/src/credentials.rs`).

Recorded rather than recomputed, and this is the step people expect to be missing. See §2.

### 4. Provisioning mints a credential for that session alone

`SessionService::credential_for` (`layer2-core/src/credentials.rs`) asks the configured `CredentialSource` for a credential minted for **this session** against **this session's service list**. A replaced runtime is re-credentialled the same way, from the same recorded list.

### 5. The sandbox is given a broker address, and an inert token

`HostedKernel::base_environment` (`layer3-adapter/src/hosted/deployment.rs`) sets, for every hosted kernel:

| Variable | Value | Why |
|---|---|---|
| `NYXID_URL` | `http://127.0.0.1:9092` | the supervisor's loopback proxy, not the platform |
| `NYXID_ACCESS_TOKEN` | `local-proxy-no-credential-needed` | a placeholder, deliberately useless |
| `CHRONO_SESSION_REACHES` | `api-github,chrono-llm` | what this session may reach, by name |

**The owner's real credential never enters the sandbox.** A terminal attachment is equivalent to shell access to that sandbox, so anything reachable from its environment is reachable by whoever is typing. The supervisor's proxy strips the placeholder and attaches the real credential on the way out, which also means a refreshed credential needs no restart.

`CHRONO_SESSION_REACHES` is always set, including empty. "This session reaches nothing" and "nothing told me" are different facts and an agent acts differently on them.

### 6. The real credential arrives over the pod's link

`SessionService::supply_credential` (`layer2-core/src/credentials.rs`) → `refresh_credential` → a `Credential` control frame over the pod's link (`layer3-adapter/src/hosted/link.rs`).

**It requires an open link**, and the link opens when the control plane attaches. `supply_credential` is called from exactly three places: `attach.rs` (a client attached), `readopt.rs` (a session re-adopted after a restart) and `credentials.rs` (a periodic sweep). The consequence is stated in `attach.rs`'s own comment: without it "the session opens, looks healthy, and refuses every model call until a sweep happens to notice minutes later".

### 7. The agent spends it

The `nyxid` CLI is a baseline package and a baseline skill (`cma/src/config.rs`, `CHRONO_BASELINE_PACKAGES` and `CHRONO_BASELINE_SKILLS`), installed in `infra/runtime/Dockerfile` with a verified checksum. It reads `NYXID_URL` and `NYXID_ACCESS_TOKEN` from the environment, so it calls the loopback proxy and never tries to refresh a credential itself.

---

## 2. What is scoped to what

- **A credential is minted per session**, against that session's own list, and re-minted the same way for a replacement runtime.
- **A session's reach is fixed when it is provisioned.** A profile can be edited afterwards — widened, narrowed, or deleted — and the session keeps what it was given. Reading the profile to learn a running session's reach gives the wrong answer precisely when somebody has changed one.
- **A rotation changes nothing about reach.** A replaced runtime is the same session.
- **The session reports its own answer.** `reach` on the session object over REST ([Sessions](./API_REST.md#3-sessions)), and `CHRONO_SESSION_REACHES` inside the sandbox. Both carry identifiers and display names; neither carries a credential.

---

## 3. What cannot be removed, and why

`Baseline` (`layer2-core/src/environment.rs`), configured at startup in `cma/src/main.rs`, carries mandated packages, mandated skills and mandated services. It is applied on every profile write and every resolution, whatever the caller sent.

These are part of what a session **is** on this platform rather than something a caller opted into. A profile that excluded the model endpoint would not be a narrower session — it would be a session that cannot think. A client that omits them still gets them, and the interface renders them as fixed rather than inviting somebody to attempt something that will be undone.

---

## 4. The three refusals

Starting a session against a service can fail for three different reasons, and a client that cannot tell them apart retries the one that will never succeed and gives up on the one that would have worked.

| `code` | Status | Means | Do |
|---|---|---|---|
| `service_not_granted` | `422` | the owner has not granted this service; `detail` names it | grant it in NyxID — **retrying will not help** |
| `service_catalogue_unavailable` | `503` | the catalogue could not be asked; the owner's services are fine | **retry** |
| — | — | this deployment has no catalogue at all | nothing — profiles still store selections and sessions still honour them |

Told apart by `code` and status, never by prose.

**The refusal is at provision, not at profile write.** A grant can be revoked between saving a profile and starting a session from it, so the write-time answer is a warning and the provision-time answer is the true one.

**A catalogue that cannot be asked refuses nothing.** What a session may reach is recorded on the profile; the catalogue exists to help choose, not to authorise. Blocking session creation on the catalogue being reachable would take the service down with it, for selections settled days ago.

---

## 5. What has been proven, and what has not

Recorded here rather than left in an issue, because a page that describes the intended path and omits what is unverified is worse than none. Full transcripts are on #131.

**Established by experiment:**

- `nyxid` resolves on `PATH` in the published runtime image, at the version the Dockerfile pins.
- `NYXID_ACCESS_TOKEN` in the sandbox is the placeholder, present from the first frame, and the real credential is not in the sandbox environment at any point.
- A sandbox whose supervisor holds no credential yet answers **`503`, body `no upstream credential available yet`** — not a `401`. An agent reasoning about a `401` will reason wrongly.
- A deployment that leaves `CHRONO_PLATFORM_BASE_URL` unset starts **no proxy at all**, while `NYXID_URL` still points at `127.0.0.1:9092` — so the sandbox sees **connection refused**, not a refusal from a broker.
- Nothing in the sandbox environment named the session's grant. That gap is what `CHRONO_SESSION_REACHES` (§1.5) closes.

**Not yet established**, and each needs a signed-in identity with a service actually granted:

- whether an agent finds the broker **unprompted** and does real work through it;
- whether the granted scope covers the calls an agent actually makes;
- what a scope refusal looks like from inside the sandbox;
- whether a credential renewal mid-session is invisible to a working agent.

Until those are walked, the honest claim is that every piece of the path is built and connected, not that the flow has been seen to work end to end.

---

## 6. The rule about values

**No credential value appears in this repository, in a log, in an activity record, or in an error.** Not in tests, not in fixtures, and nothing may be allowlisted to make a scanner pass (`CLAUDE.md` §9).

What makes that checkable rather than promised is the shape of the activity classifier (`cma/src/activity/classify.rs`): a request body is parsed into a struct with a field for each parameter somebody decided may be kept, and serde discards the rest. A field added to a request later **has nowhere to land**. A denylist would have to be updated in step with every such field and would leak the first time it was not.

The same reasoning runs through the rest of the path: the sandbox gets a placeholder rather than a credential, `reach` carries identifiers and names, `CHRONO_SESSION_REACHES` carries names, and the profile store's `Debug` keeps variable values out of logs.

---

## See also

- [Environment Profiles](./API_REST.md#2-environment-profiles) — authoring the service selection and reading the catalogue
- [Sessions](./API_REST.md#3-sessions) — the `reach` field on a session, and the three refusals as REST renders them
- `docs/API_Overview.md` §6 — rule 6, "Identity is NyxID"
- `CLAUDE.md` §4 — why a change to deployment surface changes `infra/` in the same pull request
