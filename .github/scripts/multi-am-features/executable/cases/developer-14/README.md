Developer14 owns three cases under issue #2149: Agent rename, system prompt,
and session settings. They implement the original catalog steps with actual
compiled browser actions and public AM requests. They do not start an app,
browser, compiler, cluster, or production operation.

`index.mjs` exports `cases` and `setups`. The shared author-scoped setup bridge
calls `setups[name](ctx, primitives)`. This directory implements public creation,
lifecycle/Ready checks, runtime identity checks, held-turn orchestration, and
public Sandbox retirement. It requires these low-level parent primitives:

- `primitives['configuration-runtime']` has
  `scope: 'LOCAL_DISPOSABLE_TWO_AM'`, `acquireVariant({variant,caseId})`,
  `runtimeFor({agentId})`, and `registerWitness({agentId})`.
- `acquireVariant` selects an actually configured supported or unsupported
  runtime under an exclusive fixture/account lease. It returns
  `{scope, exclusive:true, variant, runtimeImage, runtimeSource, owner, release}`.
  `runtimeImage` is an immutable digest and `runtimeSource` a full commit. The
  parent owns any configuration restoration. No workspace sharing is allowed:
  every setup creates and retires its own public lifecycle Sandbox.
- `primitives['provider-controls']` exposes the existing bounded
  `prepareTurn({caseId,kind,marker})`, `releaseTurn({caseId})`, and
  `oracle({caseId})` operations, plus
  `requestMetadata({caseId}) -> array` from the probe endpoint below. The parent
  serializes this provider's single active held case with other participants.
- The shared `browser.open({actor:'user',am,agentId})` returns the real Page,
  origin, `switchAM`, and `close`. Its evidence must retain actual request IDs,
  serving AM and owner/epoch observations. API mutations omit `am` for automatic
  non-owner routing; peer reads specify and verify their actual ingress.

Missing real capabilities throw `Blocked`; they never return an unsupported
fake AM response or synthetic case verdict. In particular, all three rows stay
honest about unavailable fixtures. The prompt row requires an actual unsupported
runtime variant; the settings row requires an alternate available model. A
single available model does not prove a model change.

For a successor parent only, select `provider_probe.py` as the provider command
while retaining the relative repository layout. It imports the existing
`cma/provider.py` and extends its handler without changing the held SSE response.
The existing model/control credentials, TLS pair and port are inherited.
Authenticated GET `/__ws4/configuration-metadata?case_id=...` returns allowlisted
model/effort values plus instruction and case hashes. The parent supplies the
HTTP bridge as `requestMetadata`; this author never reads credentials. The
probe detects prompt markers only in actual system/developer instructions,
not in the user message. This proves that a changed prompt reaches the next
model call instead of relying on a saved preferences GET.

The revision-conflict cases hold one real browser PATCH before dispatch, commit
one concurrent public mutation, then release that same PATCH. They assert 409,
disabled Save, explicit Reload preferences and durable preservation. No response
is substituted and no mutation is replayed. Rename also verifies the same
composer DOM node and unsent draft survive, plus reload/navigation/history.

Cleanup custody is registered before create. An accepted create whose request
attribution fails keeps its original lifecycle receipt and gets public cleanup.
An unknown create outcome stays a cleanup failure for parent reconciliation;
it is never rekeyed. Runtime leases release once. Cleanup checks both owner and
the lifecycle-generated Sandbox name before retiring it. Failed cleanup keeps
the row failed. The sole parent remains responsible for final scoped release.

Offline checks (not live feature acceptance):

```
node --test .github/scripts/multi-am-features/executable/cases/developer-14/contracts.test.mjs
cd .github/scripts/multi-am-features/executable/cases/developer-14
python3 -m unittest test_provider_probe.py
```

The current C275 campaign is frozen and does not load these additive cases.
Developer15 integrates the committed slice into a successor parent. Exact-image
N=3 before / N=1 after the parent's shared owner restart remains NOT_RUN until
that parent executes it; this code grants no deployment or full-matrix GO.
