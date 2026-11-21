// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { ApiError, createClient, ProtocolError, RequestAborted, TransportError } from '@cma/client';
import { createProfileViewsClient } from './profiles';

it.each([
  new ApiError(401, 'private body', 'not_authenticated', 'synthetic-request'),
  new ApiError(403, 'private body', 'forbidden', 'synthetic-request'),
  new ApiError(429, 'private body', 'rate_limited', 'synthetic-request', 3000),
  new ApiError(503, 'private body', 'profile_skills_unavailable', 'synthetic-request'),
  new ProtocolError('private body'),
  new TransportError('private body'),
  new RequestAborted(),
])(
  'forwards the exact SDK failure and request options through the skill adapter %#',
  async (failure) => {
    const sdk = createClient({ baseUrl: 'https://example.test' });
    const search = vi.spyOn(sdk.profileAuthoring, 'searchSkills').mockRejectedValueOnce(failure);
    const catalog = createProfileViewsClient(sdk).authoring!;
    const options = { cursor: 'same-page', signal: new AbortController().signal };
    await expect(catalog.searchSkills('same query', options)).rejects.toBe(failure);
    expect(search).toHaveBeenCalledExactlyOnceWith(
      { q: 'same query', cursor: 'same-page' },
      options,
    );
  },
);
