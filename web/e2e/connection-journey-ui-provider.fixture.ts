import assert from 'node:assert/strict';
import type { Route } from '@playwright/test';
import type { ConnectionMode } from './connection-journey-contract';

export const providerInput = {
  api_key: { label: 'Fixture API key', value: 'local fixture value', action: 'Connect API key' },
  device_code: { label: 'Fixture device code', value: 'LOCAL-CODE', action: 'Approve device' },
} as const;

// These forms are local controls, not implementations of provider protocols.
export function wizardProvider(
  mode: ConnectionMode,
  links: Map<string, 'pending' | 'completed'>,
  origin: string,
) {
  const submissions: { mode: ConnectionMode; accepted: boolean }[] = [];
  const route = async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const intent = url.pathname.split('/').at(-1)!;
    assert.ok(links.has(intent));
    const input = mode === 'oauth' ? undefined : providerInput[mode];
    const form =
      input === undefined
        ? `<a href="/complete/${intent}">Complete connection</a>`
        : `<form method="post" action="/complete/${intent}"><label>${input.label}<input name="value" type="${mode === 'api_key' ? 'password' : 'text'}"></label><button>${input.action}</button></form>`;
    let refused = false;
    if (url.pathname === `/complete/${intent}`) {
      const accepted =
        input === undefined
          ? request.method() === 'GET'
          : request.method() === 'POST' &&
            new URLSearchParams(request.postData() ?? '').get('value') === input.value;
      submissions.push({ mode, accepted });
      if (accepted) {
        links.set(intent, 'completed');
        return route.fulfill({
          status: 303,
          headers: { location: origin + '/connection-return?status=completed' },
        });
      }
      refused = true;
    } else assert.equal(url.pathname, `/connect/${intent}`);
    return route.fulfill({
      status: refused ? 403 : 200,
      contentType: 'text/html',
      body: `<!doctype html><title>Local ${mode} provider fixture</title>${refused ? '<p role="alert">Fixture input refused</p>' : ''}${form}`,
    });
  };
  return { route, submissions };
}
