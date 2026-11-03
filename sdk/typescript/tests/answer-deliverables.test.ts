import { expect, it } from 'vitest';
import { transport } from './resource-fixtures.js';
import { ProtocolError, RequestAborted, ValidationError } from '../src/index.js';

const item = {
  id: `delivery_${'d'.repeat(64)}`,
  reference: {
    agent_id: 'agent-1',
    resource: { kind: 'live_file', path: 'result.bin' },
    name: 'result.bin',
    media_type: 'application/octet-stream',
    size_bytes: 3,
    sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
  },
  status: 'unchecked',
} as const;
const identity = { agent_id: 'agent-1', answer_id: 'event:5', turn_sequence: 3 };
const reading = { ...identity, item };
const json = (value: unknown) =>
  new Response(JSON.stringify(value), {
    headers: { 'content-type': 'application/json' },
  });

it('downloads the selected answer version and rechecks authority after validating its bytes', async () => {
  const test = transport();
  test.answer({ ...identity, items: [item] });
  const listed = await test.client.agents.answerDeliverables('agent-1', 'event:5');
  expect(listed.items).toEqual([item]);
  test.fetch.mockResolvedValueOnce(json(reading));
  test.fetch.mockResolvedValueOnce(
    new Response('abc', {
      headers: { 'content-type': 'application/octet-stream', 'content-length': '3' },
    }),
  );
  test.fetch.mockResolvedValueOnce(json(reading));
  expect(
    await test.client.agents.downloadAnswerDeliverable('agent-1', 'event:5', listed.items[0]!),
  ).toEqual(new TextEncoder().encode('abc'));
  const root = `https://cma.example.test/api/v1/agents/agent-1/answers/event%3A5/deliverables`;
  expect(test.fetch.mock.calls.map(([url]) => String(url))).toEqual([
    root,
    `${root}/${item.id}`,
    `${root}/${item.id}/download`,
    `${root}/${item.id}`,
  ]);
});

it('accepts only a server-verified descriptor whose marker matches the explicit selected resource', async () => {
  const test = transport();
  const marker = `<!--cma-deliverable:v1:${Buffer.from(JSON.stringify(item.reference)).toString('base64url')}-->`;
  test.answer({ reference: item.reference, marker });
  expect(
    await test.client.agents.describeDeliverable('agent-1', {
      kind: 'live_file',
      path: 'result.bin',
    }),
  ).toEqual({ reference: item.reference, marker });
  expect(test.calls[0]).toMatchObject({
    url: 'https://cma.example.test/api/v1/agents/agent-1/deliverable-references',
    init: { method: 'POST', body: '{"kind":"live_file","path":"result.bin"}' },
  });
  const otherMarker = `<!--cma-deliverable:v1:${Buffer.from(JSON.stringify({ ...item.reference, name: 'someone-else.bin' })).toString('base64url')}-->`;
  test.answer({ reference: item.reference, marker: otherMarker });
  await expect(
    test.client.agents.describeDeliverable('agent-1', { kind: 'live_file', path: 'result.bin' }),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it('rejects unsafe explicit resource selectors before requesting verification', async () => {
  const test = transport();
  for (const selected of [
    { kind: 'live_file', path: '../private' },
    { kind: 'live_file', path: '/home/result.bin' },
    { kind: 'live_file', path: 'r'.repeat(1025) },
    { kind: 'agent_artifact', id: 'file:///home/result.bin' },
    { kind: 'document', id: 'doc-invalid', revision: 1 },
  ] as const) {
    await expect(
      Promise.resolve().then(() => test.client.agents.describeDeliverable('agent-1', selected)),
    ).rejects.toBeInstanceOf(ValidationError);
  }
  expect(test.fetch).not.toHaveBeenCalled();
});

it.each([
  ['corrupt bytes', 'abd', 'application/octet-stream'],
  ['oversized body', 'abcd', 'application/octet-stream'],
  ['executable response', 'abc', 'text/html'],
])('refuses %s before returning download bytes', async (_label, content, mediaType) => {
  const test = transport();
  test.fetch.mockResolvedValueOnce(json(reading));
  test.fetch.mockResolvedValueOnce(
    new Response(content, { headers: { 'content-type': mediaType } }),
  );
  await expect(
    test.client.agents.downloadAnswerDeliverable('agent-1', 'event:5', item),
  ).rejects.toBeInstanceOf(ProtocolError);
  expect(test.fetch).toHaveBeenCalledTimes(2);
});

it('discards verified bytes when independent resource access is revoked during download', async () => {
  const test = transport();
  test.fetch.mockResolvedValueOnce(json(reading));
  test.fetch.mockResolvedValueOnce(
    new Response('abc', { headers: { 'content-type': 'application/octet-stream' } }),
  );
  test.fetch.mockResolvedValueOnce(json({ ...reading, item: { ...item, status: 'denied' } }));
  await expect(
    test.client.agents.downloadAnswerDeliverable('agent-1', 'event:5', item),
  ).rejects.toBeInstanceOf(ProtocolError);
  expect(test.fetch).toHaveBeenCalledTimes(3);
});

it('aborts a pending download before a changed viewer can receive bytes', async () => {
  const test = transport();
  const cancellation = new AbortController();
  test.fetch.mockResolvedValueOnce(json(reading));
  test.fetch.mockImplementationOnce(async () => {
    cancellation.abort();
    return new Response('abc', { headers: { 'content-type': 'application/octet-stream' } });
  });
  await expect(
    test.client.agents.downloadAnswerDeliverable('agent-1', 'event:5', item, {
      signal: cancellation.signal,
    }),
  ).rejects.toBeInstanceOf(RequestAborted);
  expect(test.fetch).toHaveBeenCalledTimes(2);
});

it('rejects a foreign answer or duplicate delivery instead of showing a misleading card', async () => {
  const test = transport();
  for (const invalid of [
    { ...identity, answer_id: 'event:9', items: [item] },
    { ...identity, agent_id: 'agent-2', items: [item] },
    { ...identity, turn_sequence: 6, items: [item] },
    { ...identity, items: [item, item] },
  ]) {
    test.answer(invalid);
    await expect(
      test.client.agents.answerDeliverables('agent-1', 'event:5'),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});
