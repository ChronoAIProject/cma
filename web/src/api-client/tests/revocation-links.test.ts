// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { ApiError, createClient, pendingRevocation } from '../index.js';

const operation = 'a'.repeat(64);
const status = `</api/v1/resource-revocations/${operation}?kind=agent&id=agent>; rel="status"`;
const description = '</api/openapi.json>; rel="service-desc"; type="application/openapi+json"';
const profile = '<https://www.rfc-editor.org/rfc/rfc9457>; rel="profile"';

function pending(link: string, id = 'agent') {
  return pendingRevocation(
    new ApiError(409, 'Pending', 'resource_revocation_pending', undefined, undefined, {}, link),
    'agent',
    id,
  );
}

it('recovers the original operation when Fetch combines repeated middleware Link fields', async () => {
  const headers = new Headers({ Link: status });
  headers.append('Link', `${profile}, ${description}`);
  const fetch = vi.fn(async () =>
    Response.json(
      { code: 'resource_revocation_pending', detail: 'Pending' },
      { status: 409, headers },
    ),
  );
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const error: unknown = await client.resourceAccess
    .revokeShare('agent', 'agent', 'share', 1, { idempotencyKey: 'original-command' })
    .catch((value: unknown) => value);
  expect(error).toBeInstanceOf(ApiError);
  expect(pendingRevocation(error, 'agent', 'agent')).toBe(operation);
  expect(fetch).toHaveBeenCalledTimes(1);
});

it.each([
  `${status.padEnd(2048, ' ')}, ${status.replace(operation, 'b'.repeat(64))}`,
  `${status}; title="${'x'.repeat(2048)}"`,
])(
  'refuses the complete oversized header instead of validating a truncated prefix',
  async (link) => {
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async () =>
        Response.json(
          { code: 'resource_revocation_pending', detail: 'Pending' },
          { status: 409, headers: { Link: link } },
        ),
    });
    const error: unknown = await client.resourceAccess
      .revokeShare('agent', 'agent', 'share', 1, { idempotencyKey: 'original-command' })
      .catch((value: unknown) => value);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ code: 'resource_revocation_pending', statusLink: undefined });
    expect(pendingRevocation(error, 'agent', 'agent')).toBeUndefined();
  },
);

it('accepts a complete valid header at the capture bound', async () => {
  const link = `${status}; title="${'x'.repeat(2048 - status.length - '; title=""'.length)}"`;
  expect(link).toHaveLength(2048);
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async () =>
      Response.json(
        { code: 'resource_revocation_pending', detail: 'Pending' },
        { status: 409, headers: { Link: link } },
      ),
  });
  const error: unknown = await client.resourceAccess
    .revokeShare('agent', 'agent', 'share', 1, { idempotencyKey: 'original-command' })
    .catch((value: unknown) => value);
  expect(pendingRevocation(error, 'agent', 'agent')).toBe(operation);
});

it.each([
  status,
  `${status}, ${profile}, ${description}`,
  `${profile}, ${status}, ${description}`,
  `${description}, ${profile}, ${status}`,
  `${status}; title="Progress, still pending"`,
  `${status}; title="Progress \\"quoted\\", still pending"`,
  `<https://fixture.invalid/a,b>; rel="profile"; title="Ignore, this", ${status}`,
  status.replace('rel="status"', 'rel=status'),
  status.replace('rel="status"', 'REL="alternate STATUS"'),
  status.replace('; rel=', ' \t; \t rel = '),
])('finds a unique status relation in a valid list: %s', (link) => {
  expect(pending(link)).toBe(operation);
});

it('retains exact resource identity with URI punctuation and encoded query values', () => {
  const id = 'agent,with;punctuation&and?query#fragment';
  expect(pending(status.replace('id=agent', `id=${encodeURIComponent(id)}`), id)).toBe(operation);
  expect(pending(status.replace('id=agent', 'id=other'))).toBeUndefined();
  expect(pending(status.replace('kind=agent', 'kind=sandbox'))).toBeUndefined();
});

it.each([
  `${status}, ${status}`,
  `${status}, ${status.replace(operation, 'b'.repeat(64))}`,
  `${status}, ${status.replace('id=agent', 'id=other')}`,
  `${status}, <https://foreign.invalid/status>; rel="status"`,
  `${status}; anchor="/another-resource"`,
  `${status}; anchor=""`,
  `${status}; rel="next"`,
  `${status}; REL="status"`,
  `${status}, <https://foreign.invalid>`,
  `${status},`,
  `, ${status}`,
  `${status}, ${profile}; title="unfinished`,
  `${status}; title="unfinished\\`,
  `${status}, ${description.replace('>', '')}`,
  status.replace('</', '<https://foreign.invalid/'),
  status.replace('</', '<//foreign.invalid/'),
  status.replace('id=agent', 'id=agent#fragment'),
  status.replace('id=agent', 'id=agent&id=agent'),
  status.replace('id=agent', 'id=agent&extra=value'),
  status.replace('rel="status"', 'rel="not-status"'),
  status.replace('rel="status"', 'title="rel=status"'),
  status.replace('rel="status"', 'rel="\u00a0status"'),
  `${status}\r\n${description}`,
  `${status}; title="${'x'.repeat(8192)}"`,
  `${Array.from({ length: 32 }, () => '</p>; rel=profile').join(', ')}, ${status}`,
])('refuses ambiguous, altered or malformed status references: %s', (link) => {
  expect(pending(link)).toBeUndefined();
});
