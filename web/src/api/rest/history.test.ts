// @vitest-environment node
import { expect, it } from 'vitest';
import { restTransport } from '../../test/rest';
import { exportRawHistory } from './history';

it.each([
  'history_export_unavailable',
  'history_export_unsupported',
  'history_export_rejected',
  'instance_service_unavailable',
  'runtime_request_capacity_exhausted',
])('provides safe recovery guidance for %s without exposing upstream text', async (code) => {
  const test = restTransport();
  test.fetch.mockResolvedValue(
    new Response(JSON.stringify({ code, detail: 'private-upstream-sentinel' }), { status: 503 }),
  );
  const outcome = exportRawHistory(test.sdk, 'agt_1');
  await expect(outcome).rejects.toThrow(code);
  await expect(outcome).rejects.not.toThrow('private-upstream-sentinel');
});

it('does not reflect arbitrary problem codes or replace raw history with a transcript', async () => {
  const test = restTransport();
  test.fetch.mockImplementation(
    async () =>
      new Response(
        JSON.stringify({ code: 'private-upstream-sentinel', detail: 'private-upstream-sentinel' }),
        { status: 503 },
      ),
  );
  await expect(exportRawHistory(test.sdk, 'agt_1')).rejects.toThrow('Export retained conversation');
  expect(test.fetch).toHaveBeenCalledOnce();
  expect(test.fetch.mock.calls[0]?.[0]).toBe('/api/v1/agents/agt_1/history');
});

it('treats exhausted runtime capacity as retryable without asking the owner to change access', async () => {
  const test = restTransport();
  test.fetch.mockImplementation(
    async () =>
      new Response(JSON.stringify({ code: 'runtime_request_capacity_exhausted' }), { status: 429 }),
  );
  const outcome = exportRawHistory(test.sdk, 'agt_1');
  await expect(outcome).rejects.toThrow('Retry later');
  await expect(outcome).rejects.not.toThrow('recheck your access');
});
