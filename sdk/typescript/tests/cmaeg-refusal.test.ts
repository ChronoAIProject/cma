import { describe, expect, it } from 'vitest';
import { ProblemSchema, UpstreamRefusalSchema } from '../src/cmaeg/types.js';
import { readFileSync } from 'node:fs';

describe('CMAEG provider refusal metadata', () => {
  const legacy = { code: 'provider_rejected', status: 502, title: 'Provider rejected' };

  it('preserves legacy absence and the additive nullable extension', () => {
    expect(ProblemSchema(legacy)).toEqual(legacy);
    expect(ProblemSchema({ ...legacy, upstream: null })).toEqual({ ...legacy, upstream: null });
    for (const code of ['media_source_refused', 'unrecognized']) {
      const upstream = { operation: 'responses', status: 403, code };
      expect(UpstreamRefusalSchema(upstream)).toEqual(upstream);
      expect(ProblemSchema({ ...legacy, upstream })).toEqual({ ...legacy, upstream });
    }
  });

  it('rejects malformed provider metadata', () => {
    for (const upstream of [{}, { operation: 'responses', status: '403', code: 'unrecognized' }]) {
      expect(() => ProblemSchema({ ...legacy, upstream })).toThrow();
    }
  });

  it('accepts every published code, both operations and the HTTP status boundaries', () => {
    const contract = JSON.parse(
      readFileSync(new URL('../../../api/crates/cmaeg/openapi/v1.json', import.meta.url), 'utf8'),
    ).components.schemas.UpstreamRefusal.properties as {
      code: { enum: string[] };
      operation: { enum: string[] };
      status: { minimum: number; maximum: number };
    };
    expect(contract.operation.enum).toEqual(['responses', 'conversation']);
    expect(contract.code.enum).toHaveLength(12);
    expect(contract.status.minimum).toBe(100);
    expect(contract.status.maximum).toBe(599);
    for (const operation of contract.operation.enum) {
      for (const code of contract.code.enum) {
        for (const status of [100, 403, 599]) {
          const upstream = { operation, status, code };
          expect(UpstreamRefusalSchema(upstream)).toEqual(upstream);
          expect(ProblemSchema({ ...legacy, upstream })).toEqual({ ...legacy, upstream });
        }
      }
    }
  });

  function rejected(upstream: unknown) {
    for (const parse of [
      () => UpstreamRefusalSchema(upstream),
      () => ProblemSchema({ ...legacy, upstream }),
    ]) {
      expect(parse).toThrow();
      try {
        parse();
      } catch (error) {
        expect(String(error)).not.toContain('private');
      }
    }
  }

  it('rejects privacy canaries and every unknown code or operation without echoing them', () => {
    for (const code of [
      'private-account-identifier_42',
      'media_source_refused.private-value',
      'MEDIA_SOURCE_REFUSED',
      '',
      'x'.repeat(4096),
    ]) {
      rejected({ operation: 'responses', status: 403, code });
    }
    for (const operation of ['private-operation', 'Responses', '', 'response']) {
      rejected({ operation, status: 403, code: 'unrecognized' });
    }
  });

  it('rejects invalid status bounds, fractions and large or non-finite numbers', () => {
    for (const status of [
      -1,
      0,
      99,
      600,
      65535,
      65536,
      Number.MAX_SAFE_INTEGER,
      100.5,
      Number.NaN,
      Number.POSITIVE_INFINITY,
      Number.NEGATIVE_INFINITY,
      '403',
      null,
    ]) {
      rejected({ operation: 'responses', status, code: 'unrecognized' });
    }
  });
});
