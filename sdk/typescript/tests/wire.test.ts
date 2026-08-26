import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  GAP_VALUE_FRAMES,
  PATHS,
  PRICING_WIRE,
  PROVISIONING_STAGES,
  SESSION_STATUSES,
  STATE_DELTA_PATCHES,
  frameKey,
} from '../src/wire.js';
import { toEvent } from '../src/agui/events.js';
import { toSession } from '../src/sessions.js';
import { EMPTY, reduce } from '../src/conversation/fold.js';
import { initialState } from '../src/index.js';

/**
 * The wire, pinned.
 *
 * Both defects these cover were the client and the service each being
 * internally consistent while disagreeing with each other, and neither failed
 * loudly. A test that reads the client's own constant back to itself would
 * have passed throughout, so each of these asserts against a frame shaped the
 * way `docs/API_AGUI.md` says the service shapes it.
 */

describe('a STATE_DELTA carries its patches under the name the service uses', () => {
  it('reads them, rather than finding nothing', () => {
    // The defect: this client read `patch` while the service sends `delta`,
    // so every STATE_DELTA arrived carrying an empty list and applied to
    // nothing. No session ever reported what it had spent.
    expect(STATE_DELTA_PATCHES).toBe('delta');

    const event = toEvent({
      type: 'STATE_DELTA',
      threadId: 'sesn_1',
      runId: 'run_1',
      seq: 4,
      delta: [{ op: 'replace', path: PATHS.inputTokens, value: 120 }],
    });

    expect(event.type).toBe('STATE_DELTA');
    if (event.type !== 'STATE_DELTA') {
      return;
    }
    expect(event.patch).toHaveLength(1);
  });
});

describe('the canonical AG-UI initial state', () => {
  it('is the exact document published by the service contract', () => {
    const document = readFileSync(new URL('../../../docs/API_AGUI.md', import.meta.url), 'utf8');
    const marker = '### Canonical initial state\n\n```json\n';
    const published = document.split(marker)[1]?.split('\n```')[0];

    expect(published, 'docs/API_AGUI.md has no canonical initial-state block').toBeDefined();
    expect(initialState()).toEqual(JSON.parse(published ?? 'null'));
  });

  it('returns fresh arrays for every session fold', () => {
    const first = initialState();
    first.gaps.push({ frames: 1 });
    first.usage.inputTokens = 99;

    expect(initialState().gaps).toEqual([]);
    expect(initialState().usage.inputTokens).toBeNull();
  });

  it('keeps unreported usage and commands distinct from zero and empty reports', () => {
    const state = initialState();

    expect(state.usage).toEqual({
      inputTokens: null,
      outputTokens: null,
      contextUsed: null,
      contextWindow: null,
    });
    expect(state.availableCommands).toBeNull();
    expect(state.pendingApprovals).toEqual({});
    expect(state.status).toBeNull();
    expect(state.progress).toBeNull();
  });

  it('pins the Layer 2 status and provisioning vocabularies', () => {
    expect(SESSION_STATUSES).toEqual([
      'provisioning',
      'ready',
      'running',
      'rotating',
      'idle',
      'terminated',
    ]);
    expect(PROVISIONING_STAGES).toEqual([
      'accepted',
      'runtime_requested',
      'runtime_created',
      'skills_installed',
      'attaching',
      'ready',
    ]);
    expect(PATHS.status).toBe('/status');
    expect(PATHS.progress).toBe('/progress');
  });
});

describe('a conversation gap carries the field the service writes', () => {
  it('pins the value shape directly to the service projection', () => {
    const source = readFileSync(
      new URL('../../../api/crates/layer1-public/src/agui/project.rs', import.meta.url),
      'utf8',
    );
    const projection = /EventKind::ConversationGap[\s\S]*?ending: None,\n\s*},/.exec(source)?.[0];
    expect(projection).toBeDefined();
    expect(projection).toContain(`"path": "${PATHS.gaps}"`);
    expect(projection).toContain(`"value": { "${GAP_VALUE_FRAMES}": frames }`);

    const asTheServiceWritesIt = { frames: 7 };
    expect(GAP_VALUE_FRAMES).toBe('frames');
    expect(Object.keys(asTheServiceWritesIt)).toEqual([GAP_VALUE_FRAMES]);

    const state = reduce(
      EMPTY,
      toEvent({
        type: 'STATE_DELTA',
        threadId: 'sesn_1',
        runId: 'run_1',
        seq: 4,
        delta: [{ op: 'add', path: '/gaps/-', value: asTheServiceWritesIt }],
      }),
    );
    expect(state.entries[0]).toMatchObject({ kind: 'gap', frames: 7 });
  });
});

describe('pricing uses the names declared by the REST contract', () => {
  it('pins the quote, create claim, and session projection in one vocabulary', () => {
    expect(PRICING_WIRE).toEqual({
      quotePath: '/api/v1/pricing/quote',
      spend: 'spend',
      environmentProfile: 'environment_profile',
      quotedRateCard: 'quoted_rate_card',
      pricing: 'pricing',
      priced: 'priced',
      rateCard: 'rate_card',
      currency: 'currency',
      cpuMillis: 'cpu_millis',
      memoryMib: 'memory_mib',
      microsPerHour: 'micros_per_hour',
      accruedMicros: 'accrued_micros',
      display: 'display',
      billableSeconds: 'billable_seconds',
      accruing: 'accruing',
      unboundedIntervals: 'unbounded_intervals',
      per: 'per',
      quotedAt: 'quoted_at',
    });
  });
});

describe('usage lands where the service writes it', () => {
  it('folds the counts the service actually sends', () => {
    // The defect: the paths were spelled `/usage/tokens` and `/usage/cost`
    // here while the service writes `/usage/inputTokens`,
    // `/usage/outputTokens`, `/usage/contextUsed` and
    // `/usage/contextWindow`, so every patch fell through to the ignored
    // branch.
    expect(PATHS.inputTokens).toBe('/usage/inputTokens');
    expect(PATHS.outputTokens).toBe('/usage/outputTokens');
    expect(PATHS.contextUsed).toBe('/usage/contextUsed');
    expect(PATHS.contextWindow).toBe('/usage/contextWindow');

    const frames = [
      { op: 'replace', path: PATHS.inputTokens, value: 1200 },
      { op: 'replace', path: PATHS.outputTokens, value: 300 },
      { op: 'replace', path: PATHS.contextUsed, value: 44 },
      { op: 'replace', path: PATHS.contextWindow, value: 200_000 },
    ];
    const state = reduce(
      EMPTY,
      toEvent({
        type: 'STATE_DELTA',
        threadId: 'sesn_1',
        runId: 'run_1',
        seq: 5,
        delta: frames,
      }),
    );

    expect(state.usage?.inputTokens).toBe(1200);
    expect(state.usage?.outputTokens).toBe(300);
    expect(state.usage?.contextUsed).toBe(44);
    expect(state.usage?.contextWindow).toBe(200_000);
    // §18.5 asks for the count a reader watches, not a breakdown to add up.
    expect(state.usage?.tokens).toBe(1500);
  });

  it('reports no total until the service has reported one', () => {
    // §18.5: absence is a fact, not a zero. A total shown before anything
    // arrived would be this client inventing a figure.
    expect(EMPTY.usage?.tokens).toBeUndefined();
  });
});

describe('one durable event may project to several frames', () => {
  it('keys them apart, so none is mistaken for a duplicate', () => {
    // A committed message becomes START, then CONTENT, then END, and all
    // three carry the sequence of the event they came from — that sequence
    // is the cursor a reconnect resumes on. Keyed by sequence alone, the
    // siblings were dropped and every message rendered empty.
    expect(frameKey(7, 'TEXT_MESSAGE_START')).not.toBe(frameKey(7, 'TEXT_MESSAGE_CONTENT'));
    // The overlap the dedup exists for still collapses.
    expect(frameKey(7, 'TEXT_MESSAGE_CONTENT')).toBe(frameKey(7, 'TEXT_MESSAGE_CONTENT'));
  });
});

describe('the paths match what the service emits', () => {
  /*
   * The list on the right was read out of `api/crates/layer1-public/src/agui/`
   * on 2026-08-19, and checked again on 2026-08-26, by listing every `"path"`
   * the projection writes. It is the whole set, so this catches both halves of the drift that caused the
   * original defect: a path declared here that nothing writes, and a path the
   * service writes that nothing here reads.
   *
   * `/usage/cost` is the one deliberate exception and is asserted as such
   * below — declared, unwritten, and recorded as unwritten.
   */
  const EMITTED = [
    '/availableCommands',
    '/configOptions',
    '/contextCompactions/-',
    '/gaps/-',
    '/memoryPressure',
    '/mode',
    '/pendingApprovals/{requestId}',
    '/plan',
    '/planeChanges/-',
    '/progress',
    '/resources',
    '/runtimeLosses/-',
    '/signIn',
    '/status',
    '/usage/contextUsed',
    '/usage/contextWindow',
    '/usage/inputTokens',
    '/usage/outputTokens',
  ];

  it('declares every path the service writes', () => {
    const declared = new Set(Object.values(PATHS));
    for (const path of EMITTED) {
      expect(declared, `the service emits ${path} and nothing here reads it`).toContain(path);
    }
  });

  it('declares nothing beyond those, but for the one recorded absence', () => {
    // A path declared here that the service never writes is a promise this
    // client cannot keep, and the reason the usage panel read zero for months.
    const extra = Object.values(PATHS).filter((path) => !EMITTED.includes(path));
    expect(extra).toEqual(['/usage/cost']);
  });
});

describe("a session's agent configuration is read as the service writes it", () => {
  /*
   * The other half of `a_view_carries_the_agent_configuration_a_client_draws`
   * in `api/crates/layer1-public/src/rest/mod_tests.rs`. That test asserts the
   * service emits this document; this one asserts the client reads it.
   *
   * The document is written out in full rather than built from the client's own
   * constants, for the reason this whole file exists: a client reading its own
   * names back to itself stays green while disagreeing with the service, which
   * is how every defect above shipped.
   */
  const asTheServiceSendsIt = {
    id: 'sesn_1',
    status: 'ready',
    recovery_state: 'normal',
    kernel: 'codex',
    cols: 120,
    rows: 40,
    last_seq: 3,
    created_at: '2026-08-12T00:00:00Z',
    conforms_to: [],
    reach: [],
    agent: {
      model: 'gpt-5-codex',
      reasoning_effort: 'x_high',
      permission: 'unrestricted',
      credential: 'own',
      skills: [{ id: 'skl_nyxid' }],
    },
  };

  it('reads every field, under the name the service uses', () => {
    const session = toSession(asTheServiceSendsIt);
    expect(session.agent?.model).toBe('gpt-5-codex');
    // `x_high` and not `xhigh`. The service's `as_str` spells it the other way
    // and only serde reaches this client.
    expect(session.agent?.reasoningEffort).toBe('x_high');
    expect(session.agent?.permission).toBe('unrestricted');
    expect(session.agent?.credential).toBe('own');
    expect(session.agent?.skills).toEqual([
      { id: 'skl_nyxid', name: undefined, description: undefined },
    ]);
  });

  it('drops a word it has no rendering for, rather than printing it', () => {
    // A newer service naming a mode this build does not know. Absent draws as
    // "not reported", which is honest; passing it through would print a raw
    // wire token into a panel somebody reads.
    const session = toSession({
      ...asTheServiceSendsIt,
      agent: { ...asTheServiceSendsIt.agent, permission: 'something-new' },
    });
    expect(session.agent?.permission).toBeUndefined();
    // And only that field. The rest of the document still reads.
    expect(session.agent?.credential).toBe('own');
  });

  it('tells a service that said nothing from one that said none', () => {
    const older = toSession({ ...asTheServiceSendsIt, agent: undefined });
    expect(older.agent).toBeUndefined();

    const withoutSkills = { ...asTheServiceSendsIt.agent, skills: undefined };
    const unrecorded = toSession({ ...asTheServiceSendsIt, agent: withoutSkills });
    expect(unrecorded.agent?.skills).toBeUndefined();

    const none = toSession({
      ...asTheServiceSendsIt,
      agent: { ...asTheServiceSendsIt.agent, skills: [] },
    });
    expect(none.agent?.skills).toEqual([]);
  });

  it('reads the defaults the service always names', () => {
    // The service sends these on every session rather than omitting them, so a
    // client must not treat an unconfigured session as an unreported one.
    const session = toSession({
      ...asTheServiceSendsIt,
      agent: { permission: 'guarded', credential: 'delegated', reasoning_effort: 'high' },
    });
    expect(session.agent?.permission).toBe('guarded');
    expect(session.agent?.credential).toBe('delegated');
    expect(session.agent?.reasoningEffort).toBe('high');
    expect(session.agent?.model).toBeUndefined();
  });
});
