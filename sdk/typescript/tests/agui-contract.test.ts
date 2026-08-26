import { readdirSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  PATHS,
  applyStateDelta,
  initialState,
  parseEventStream,
  type AguiEvent,
} from '../src/index.js';

const LOCAL = new URL('agui-fixtures/', import.meta.url);
const SERVER = new URL(
  '../../../api/crates/layer1-public/tests/fixtures/agui-sdk/',
  import.meta.url,
);

function localFixture(name: string): string {
  return readFileSync(new URL(name, LOCAL), 'utf8');
}

function enabledEvents(source: string): string[] {
  const document = JSON.parse(source) as unknown;
  if (typeof document !== 'object' || document === null || Array.isArray(document)) {
    throw new Error('AG-UI capability fixture was not an object');
  }
  const events = (document as Record<string, unknown>).events;
  if (typeof events !== 'object' || events === null || Array.isArray(events)) {
    throw new Error('AG-UI capability fixture had no events map');
  }
  const entries = Object.entries(events);
  if (entries.some(([, enabled]) => typeof enabled !== 'boolean')) {
    throw new Error('AG-UI capability fixture had a non-boolean event declaration');
  }
  return entries
    .filter(([, enabled]) => enabled)
    .map(([kind]) => kind)
    .sort();
}

function eventKind(event: AguiEvent): string {
  return event.type === 'UNKNOWN' ? event.kind : event.type;
}

function assertVocabulary(capability: string, projection: string): AguiEvent[] {
  const expected = enabledEvents(capability);
  const events = parseEventStream(projection);
  const observed = [...new Set(events.map(eventKind))].sort();
  const missing = expected.filter((kind) => !observed.includes(kind));
  const unexpected = observed.filter((kind) => !expected.includes(kind));
  if (missing.length > 0 || unexpected.length > 0) {
    throw new Error(
      `AG-UI fixture vocabulary diverged: missing [${missing.join(', ')}]; unexpected [${unexpected.join(', ')}]`,
    );
  }
  return events;
}

describe('the server-owned AG-UI contract fixtures', () => {
  it('keeps the SDK copies byte-identical to the Rust-owned corpus', () => {
    const localNames = readdirSync(LOCAL).sort();
    const serverNames = readdirSync(SERVER).sort();
    expect(localNames).toEqual(serverNames);

    for (const name of serverNames) {
      expect(readFileSync(new URL(name, LOCAL))).toEqual(readFileSync(new URL(name, SERVER)));
    }
  });

  it('replays every declared emitted kind through the real SDK parser', () => {
    const events = assertVocabulary(
      localFixture('capabilities.json'),
      localFixture('projection.sse'),
    );

    expect(events.some((event) => event.type === 'UNKNOWN')).toBe(false);
    expect(events.find((event) => event.type === 'TOOL_CALL_ARGS')).toMatchObject({
      toolCallId: 'call_fixture',
      delta: '{"path":"docs/API_AGUI.md"}',
      truncated: false,
    });
    expect(events.find((event) => event.type === 'REASONING_MESSAGE_CONTENT')).toMatchObject({
      ephemeral: true,
      seq: 0,
    });

    const usage = events.find((event) => event.type === 'STATE_DELTA');
    expect(usage?.type).toBe('STATE_DELTA');
    if (usage?.type !== 'STATE_DELTA') {
      throw new Error('the server-owned fixture carried no STATE_DELTA');
    }
    expect(usage.patch).toEqual([
      { op: 'replace', path: PATHS.inputTokens, value: 120 },
      { op: 'replace', path: PATHS.outputTokens, value: 30 },
      { op: 'replace', path: PATHS.contextUsed, value: 4_096 },
      { op: 'replace', path: PATHS.contextWindow, value: 200_000 },
    ]);
    expect(applyStateDelta(initialState(), usage.patch).usage).toEqual({
      inputTokens: 120,
      outputTokens: 30,
      contextUsed: 4_096,
      contextWindow: 200_000,
    });
  });

  it('names the divergent event when a fixture is corrupted', () => {
    const projection = localFixture('projection.sse');
    const corrupted = projection.replaceAll('TOOL_CALL_RESULT', 'TOOL_CALL_RESULT_DRIFT');
    expect(corrupted).not.toBe(projection);

    expect(() => assertVocabulary(localFixture('capabilities.json'), corrupted)).toThrowError(
      /missing \[TOOL_CALL_RESULT\].*unexpected \[TOOL_CALL_RESULT_DRIFT\]/,
    );
  });
});
