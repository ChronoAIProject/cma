import { expect, type Request } from '@playwright/test';

export function verifiedEntryHeaders(request: Request, subject: string): Record<string, string> {
  if (!/^\/api\/v2\/triggers\/public\/[^/]+\/launches$/.test(new URL(request.url()).pathname))
    return {};
  const verified = `v1.${Buffer.from(subject).toString('base64url')}`;
  expect(request.headers()['x-cma-expected-subject']).toBe(verified);
  return { 'X-CMA-Verified-Subject': verified };
}
