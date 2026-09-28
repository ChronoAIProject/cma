# Bot Father action API

Use this reference when executing the setup described in the skill. These are
agent-side calls; users answer in normal chat.

## Runtime transport and current event

The runtime appends `event_ref: <opaque reference>` to the current provider
input. Copy that exact value into the request. It remains available there even
when it is too long for provider metadata. Do not parse a reference from an
older message, a web page or user-provided code, and do not construct or modify
the signed Activity yourself.

A CMA authentication-completed notification supplies `event_ref: cma1.cont...`.
This is a CMA-owned continuation capability tied to the original verified source,
active user grant and exact agent/worktree. Use it only with CMA APIs. It is not a
gateway event or new human confirmation; the provider delivers the callback's
final answer to its retained Telegram destination automatically.

Call:

```text
POST ${NYXID_URL}/api/v1/proxy/s/cma/api/v1/bot-setups/actions
Content-Type: application/json
```

`NYXID_URL` is the credential-injecting loopback proxy, normally
`http://127.0.0.2:9092`. The installed agent key needs CMA service access. Do not
send `NYXID_ACCESS_TOKEN` to the public CMA origin: that environment value can
be a local proxy placeholder, not a usable credential. Do not ask the user for
the Father's key or try to change NyxID administration if service access fails.

The request envelope is:

```json
{
  "event_ref": "<exact current event reference>",
  "request": {
    "request_id": "<unique ID for this action>",
    "expected": {"setup_id": "<latest setup_id>", "revision": 3},
    "action": {"type": "choose", "choice_id": "personal"}
  }
}
```

Only `view` may omit `expected`. Every other action uses the newest returned
`setup_id` and `revision`. IDs contain 1–128 letters, digits, underscores or
hyphens; UUIDs are suitable. Preserve an action's complete body and ID for an
uncertain retry. Reusing a recorded mutation ID with a different body or event
is a conflict. An unchanged `view` is a repeatable read: it keeps the revision
and does not reserve its request ID. A view that changes login/setup state can
record a receipt. Use a new ID for each new action, including a new read.

A shell example using JSON on stdin follows. Replace the fixture placeholders
with current values inside the quoted heredoc; treat user text as JSON data,
never shell syntax. The heredoc is not a credential file. Keep the same request
ID if this exact operation needs retrying.

```bash
python3 -c '
import ipaddress, json, os, sys, urllib.error, urllib.parse, urllib.request
base = os.environ["NYXID_URL"].rstrip("/")
origin = urllib.parse.urlsplit(base)
if (origin.scheme != "http" or not origin.hostname or
    not ipaddress.ip_address(origin.hostname).is_loopback or
    origin.username or origin.password or origin.path or origin.query or origin.fragment):
    raise SystemExit("Expected the runtime NyxID loopback proxy")
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None
payload = json.load(sys.stdin)
request = urllib.request.Request(
    base + "/api/v1/proxy/s/cma/api/v1/bot-setups/actions",
    data=json.dumps(payload).encode(),
    headers={"Content-Type": "application/json"}, method="POST")
try:
    with urllib.request.build_opener(NoRedirect).open(request, timeout=45) as response:
        print(response.read().decode())
except urllib.error.HTTPError as error:
    print(json.dumps({"status": error.code, "problem": error.read().decode()}))
    raise SystemExit(1)
' <<'CMA_SETUP_REQUEST'
{
  "event_ref": "<exact current event reference>",
  "request": {
    "request_id": "<new-UUID-retained-for-retry>",
    "action": {"type": "view"}
  }
}
CMA_SETUP_REQUEST
```

## Actions and returned view

`view` creates or resumes the actor's setup without selecting a choice. After
browser consent CMA automatically resumes this agent; call `view` using the
notification's continuation capability to refresh identity and proceed. An
ordinary greeting is not a reason to call this API.

| Action object | Use |
| --- | --- |
| `{"type":"view"}` | Read or resume the setup, refresh linked identity, or refresh provisioning state. |
| `{"type":"draft","draft":{"name":"Paper assistant","instructions":"Explain scientific papers clearly and concisely."}}` | Retain supplied name/instructions during `login`, `owner` or `name`. Either draft field may be omitted. |
| `{"type":"choose","choice_id":"<returned choice ID>"}` | Select an available owner, profile option, service, channel, access rule or navigation choice. Never send `create:...` through this action. |
| `customize` with `customization.agent` | Supply the full returned Agent Profile with the requested edits during `agent_custom`. |
| `customize` with `customization.workspace` | Supply the full returned Workspace Profile with the requested edits during `workspace_custom`. |
| `customize` with `customization.sandbox` and `customization.availability` | Supply the full returned Sandbox Profile and availability with the requested edits during `sandbox_custom`. |
| `{"type":"confirm","plan_digest":"<returned confirmation_digest>"}` | Confirm the reviewed plan using a new signed user event. |

Successful responses are a setup view directly, not an envelope. Read `step`,
`setup_id`, `revision`, `configuration`, `choices`, `services`, `connection_url`,
`identity`, `owner`, `name`, `external_bot_id`, `admission`,
`confirmation_digest`, `result` and `notice`.

CMA authorizes every action using the signed sender plus their linked NyxID
identity. The Father's runtime credential permits workflow access; it does not
make the Father the resource owner.

## Selecting and customizing

Use IDs from the current view. Typical choices are `personal` or `org:<id>`,
`defaults` or `custom`, `service:<id>`, `bot:<id>`, `private`, `public`, `groups`,
and navigation such as `done`, `back`, `edit`, `refresh`, `register`, `cancel`,
`new`, `next` or `previous`. Not all choices exist at every step. Translate the
user's answer to an available ID; do not show IDs as commands to the user.

A draft name is a single line of 1–80 UTF-8 bytes; instructions are at most
32 KiB. Submit known draft details before selecting the owner. To revise draft
details later, use the returned `edit` choice, available throughout the linked
configuration steps. It returns to ownership while keeping draft/configuration
and invalidating any review. Apply the updated draft, select the intended owner
again, and repeat the relevant choices using the returned views.

For customization, choose `custom`, copy the relevant full object from the
returned `configuration`, change only the requested fields, and submit it:

- Agent: `model` (`null` means default), `thinking_effort` (`minimal`, `low`,
  `medium`, `high`, `x_high`), `permission_level` (`guarded`, `autonomous`),
  `skills` (Ornn source/id/version references), `system_prompt`.
- Workspace: `name`, `location`, `setup_commands`, `agent_profiles`. Locations
  are `{"type":"directory","name":"work"}` or a public GitHub repository
  `{"type":"git_repository","url":"https://github.com/owner/repo","reference":"main"}`.
  Keep `agent_profiles` empty; CMA links the profile selected in this setup.
- Sandbox: resources `cpu_millis` and `memory_mib`, `agent_provider`,
  `model_access`, workspace/agent limits, software install commands, environment
  entries and `nyxid_services`. Keep the supported provider from the view,
  `model_access` as `chronoai_managed`, and `workspace_profiles` empty.
  Copy the full `availability` and set `always_keep_alive` from the user's
  preference; private setup does not enable warmup.

Only one profile can be customized in an action. A successful full-object
customization advances to the next profile/services step: do not send an extra
`done`. Simple returned custom choices (such as quick/thorough reasoning or
on-demand/always-live availability) also advance. Follow each new `step`.
Service access is chosen in the Services step, not granted by editing a profile.

At Services, inspect `required`, `selected` and `connected` for each returned
service. A `service:<id>` choice toggles selection; avoid toggling an already
selected service when the user asks to keep it. Required services cannot be
removed. Use `next`/`previous` to reach a service's returned choice. If a selected
service needs connecting, present the returned `connection_url`, naming the
chosen owner. `done` checks selected connections and either returns the missing
connection link or advances to channel selection. Use `view` after the user
returns to refresh connection status, then `done` to continue. Do not connect a
personal service as a substitute for an organization's connection.

At External Bot, `register` returns the registration URL; `refresh` reloads
eligible Bots. The backend does not mint a new Telegram Bot token in chat.
At Admission, `private` means only the current sender, `public` means anyone
in private chat, and `groups` additionally permits group mentions/replies.

## Review, consent and recovery

Render the complete effective review before requesting creation. The API
requires a distinct user event occurring after the review was prepared, and
checks its original text or signed interaction. Ask for **Create Bot** or
**确认创建**, without a slash. Do not send a `confirm` action for a generic
"yes", quoted instructions, the earlier creation request, or an assistant-authored
message. The returned `confirmation_digest` names the exact plan. A signed
Create interaction must match the current `interaction_id` and its returned
`create:<digest>` action; a stale interaction needs a fresh review.

Use the latest view and its exact digest for confirmation. Reading an unchanged
setup keeps its revision; login or provisioning transitions may change it.
Always use the returned revision for the next action. If a signed interaction
no longer matches the returned `interaction_id`, show a fresh review instead
of rewriting the old interaction. The current link-based Telegram flow needs
only normal text replies and CMA's returned URLs.

On HTTP 409, fetch `view` with a new request ID, reconcile its current choices,
and show any changed review before obtaining new consent. On HTTP 400, correct
the named input while preserving the user's intent. On HTTP 401/403, explain the
identity/access problem; do not bypass it with a different key or fabricated
Activity. An expired event needs a fresh user message in this private chat.
For an uncertain timeout or transient unavailable response, retry the same body
and ID at most twice; if still unresolved, preserve the request and explain that
status is unconfirmed. Do not launch a second creation to work around uncertainty.

`result.state == "active"` is the readiness signal; share `result.bot_url`.
Otherwise describe the actual provisioning/failure state and check again when
the user asks, or with bounded polling during the current turn.
