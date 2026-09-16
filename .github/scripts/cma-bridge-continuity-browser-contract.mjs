import { createHash } from 'node:crypto';
import { validateOwnedFixture } from './cma-bridge-continuity-owned-config.mjs';
const check = (condition, code) => { if (!condition) throw new Error(code); };
export const hash = (value) => createHash('sha256').update(value).digest('hex');

export function scope(config, journal, result, change, recovery) {
  check(/^[a-z0-9][a-z0-9-]{0,47}$/.test(change), 'bounded_change_identity_required');
  validateOwnedFixture(config, journal, result, recovery);
  const agentId = journal.resources.agentIds[1];
  return { origin: config.origin, run: config.run, owner: config.expectedSubject, agentId,
    resources: journal.resources, originalMarker: config.markers.manual,
    activeMarker: `CMA-ACTIVE-${change}`, finalMarker: `${config.markers.manual} CMA-ACTIVE-${change}` };
}

export function classify(urlString, owned) {
  const url = new URL(urlString);
  if (url.origin !== owned.origin) return { path: url.pathname, origin: url.origin, kind: 'external' };
  const value = { path: url.pathname, origin: url.origin, kind: 'read' };
  if (url.pathname === `/api/v2/agents/${owned.agentId}/events`) value.kind = 'v2_agent_sse';
  if (url.pathname === '/api/v1/agui/events' && url.searchParams.get('thread_id') === owned.agentId) value.kind = 'legacy_agui_sse';
  const cursor = url.searchParams.get('after_cursor') ?? url.searchParams.get('starting_after');
  if (value.kind.endsWith('_sse') && cursor !== null) value.cursorSha256 = hash(cursor);
  return value;
}

export function compareUi(before, after, owned) {
  const historyPreserved = before.messages.every((value, index) => JSON.stringify(value) === JSON.stringify(after.messages[index]));
  const sameDocument = after.documentId === before.documentId && after.timeOrigin === before.timeOrigin
    && after.navigationCount === before.navigationCount && after.navigationEntries === before.navigationEntries;
  const sameAgent = after.path === `/agents/${owned.agentId}` && before.path === after.path;
  const noMutation = after.allowedMutations === 0 && after.blockedMutations === 0;
  return { historyPreserved, sameDocument, sameAgent, noMutation,
    finalMarkerVisible: after.finalMarkerVisible, documentHealthy: !after.closed,
    status: historyPreserved && sameDocument && sameAgent && noMutation && after.finalMarkerVisible && !after.closed ? 'passed' : 'failed' };
}

export function networkAssessment(requests, events, armedAt, through) {
  const streams = requests.filter((value) => value.kind.endsWith('_sse'));
  const connected = streams.filter((value) => value.status === 200 && value.sse === true);
  const start = Date.parse(armedAt);
  const end = Date.parse(through);
  const endings = streams.filter((value) => {
    const at = Date.parse(value.failedAt ?? value.finishedAt ?? '');
    return at >= start && at <= end;
  });
  const reopened = endings.flatMap((previous) => {
    const at = previous.failedAt ?? previous.finishedAt;
    const next = connected.find((value) => value.id !== previous.id && value.kind === previous.kind
      && Date.parse(value.startedAt) >= Date.parse(at) && Date.parse(value.startedAt) <= end);
    return next ? [{ previous: previous.id, next: next.id, closedAt: at, reopenedAt: next.startedAt,
      closure: previous.failedAt ? 'network_failure' : 'stream_closed', nextCursorSha256: next.cursorSha256 }] : [];
  });
  return { observedTransports: [...new Set(connected.map((value) => value.kind))],
    streamConnections: connected.length, automaticReopens: reopened,
    httpFailures: requests.filter((value) => value.status >= 400).map(({ id, kind, status, startedAt }) => ({ id, kind, status, startedAt })),
    networkFailures: requests.filter((value) => value.failedAt).map(({ id, kind, failedAt, failure }) => ({ id, kind, failedAt, failure })),
    streamErrors: events.filter((value) => value.selected && (value.type === 'stream_read_error'
      || (value.type === 'stream_frame' && ['cma.agent.error', 'RUN_ERROR'].includes(value.frameType)))),
    reconnectEvidence: reopened.length ? 'observed' : 'not_observed',
    transportCoverage: connected.length ? 'fetch_sse_observed' : 'get_observations_only_no_stream_proof' };
}
