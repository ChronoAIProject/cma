import { readFileSync } from 'node:fs';
import type { BridgeSourceControlReceipt } from '../src/index.js';
const corpus = JSON.parse(
  readFileSync(
    new URL(
      '../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-source-consent.json',
      import.meta.url,
    ),
    'utf8',
  ),
) as { cases: { id: string; response: { body: unknown } }[] };
export function receipt() {
  return structuredClone(
    corpus.cases.find((item) => item.id === 'source-consent-enable')!.response.body,
  ) as BridgeSourceControlReceipt;
}
export function response(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': status >= 400 ? 'application/problem+json' : 'application/json' },
  });
}
export function edit(pointer: string, value: unknown) {
  const body = structuredClone(receipt()) as unknown as Record<string, unknown>;
  const parts = pointer.split('/').filter(Boolean);
  let parent = body;
  for (const key of parts.slice(0, -1)) parent = parent[key] as Record<string, unknown>;
  parent[parts.at(-1)!] = value;
  return body;
}
