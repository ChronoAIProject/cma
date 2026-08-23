import { describe, expect, it, vi } from 'vitest';
import { resolveConfig } from './config';
import { createSession, quoteSession, readAllowance, readSpend } from './sessions';

/** @vitest-environment node */

function answering(body: unknown) {
  const fetch = vi.fn(() =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: () => Promise.resolve(body),
    } as Response),
  );
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls: fetch };
}

describe('the live-session allowance', () => {
  it('reads the caller-scoped advisory response', async () => {
    const { fetch, calls } = answering({ limit: 3, live: 2, exempt: false });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      limit: 3,
      live: 2,
      exempt: false,
    });
    expect(calls).toHaveBeenCalledWith(
      '/api/v1/sessions/allowance',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('keeps an unconfigured limit absent', async () => {
    const { fetch } = answering({ live: 7, exempt: true });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      live: 7,
      exempt: true,
    });
  });

  it('treats malformed advisory fields as permissive defaults', async () => {
    const { fetch } = answering({ limit: 0, live: 'many', exempt: 'yes' });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      live: 0,
      exempt: false,
    });
  });
});

describe('capacity pricing', () => {
  const quote = {
    rate_card: 'rate_exact',
    currency: 'USD',
    cpu_millis: 1000,
    memory_mib: 1024,
    micros_per_hour: '45000',
    display: '$0.045',
    per: 'hour',
    quoted_at: '2026-08-23T05:00:00Z',
  };

  it('reads a profile quote and preserves the server display verbatim', async () => {
    const { fetch, calls } = answering(quote);
    await expect(quoteSession(resolveConfig({ fetch }), 'envp_1')).resolves.toEqual({
      priced: true,
      rateCard: 'rate_exact',
      currency: 'USD',
      cpuMillis: 1000,
      memoryMib: 1024,
      microsPerHour: '45000',
      display: '$0.045',
      per: 'hour',
      quotedAt: '2026-08-23T05:00:00Z',
    });
    expect(calls).toHaveBeenCalledWith(
      '/api/v1/pricing/quote?environment_profile=envp_1',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('keeps an unpriced quote distinct from a zero quote', async () => {
    const { fetch } = answering({ priced: false });
    await expect(quoteSession(resolveConfig({ fetch }), null)).resolves.toEqual({ priced: false });
  });

  it('sends the exact card the owner confirmed when creating', async () => {
    const { fetch, calls } = answering({
      id: 'sesn_1',
      kernel: 'codex',
      status: 'provisioning',
      created_at: '2026-08-23T05:00:00Z',
      conforms_to: [],
    });
    await createSession(resolveConfig({ fetch }), {
      cols: 80,
      rows: 24,
      environmentProfile: 'envp_1',
      quotedRateCard: 'rate_exact',
    });

    const [, init] = calls.mock.calls[0] as unknown as [RequestInfo | URL, RequestInit];
    expect(JSON.parse(String(init.body))).toEqual({
      cols: 80,
      rows: 24,
      environment_profile: 'envp_1',
      quoted_rate_card: 'rate_exact',
    });
  });

  it('reads spend without coercing either monetary integer through a number', async () => {
    const { fetch, calls } = answering({
      priced: true,
      rate_card: 'rate_exact',
      currency: 'USD',
      micros_per_hour: '45000',
      accrued_micros: '90000',
      display: '$0.09',
      billable_seconds: 7200,
      accruing: false,
      unbounded_intervals: 1,
    });
    await expect(readSpend(resolveConfig({ fetch }), 'sesn/one')).resolves.toEqual({
      priced: true,
      rateCard: 'rate_exact',
      currency: 'USD',
      microsPerHour: '45000',
      accruedMicros: '90000',
      display: '$0.09',
      billableSeconds: 7200,
      accruing: false,
      unboundedIntervals: 1,
    });
    expect(calls).toHaveBeenCalledWith(
      '/api/v1/sessions/sesn%2Fone/spend',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('refuses a monetary JSON number rather than silently rounding it', async () => {
    const { fetch } = answering({ ...quote, micros_per_hour: 45000 });
    await expect(quoteSession(resolveConfig({ fetch }))).rejects.toThrow('invalid micros_per_hour');
  });
});
