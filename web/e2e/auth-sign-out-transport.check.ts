import { createServer, get } from 'node:http';
import { expect, test, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const noticeKey = 'cma.auth.sign-out';
const signedIn = {
  authenticated: true,
  subject: 'local-owner',
  display_name: 'Local owner',
  capabilities: ['use_own_sandboxes', 'create_sandbox'],
};

async function logoutServer(preview: string, logoutStatus: number, landingStatus: number) {
  const upstream = new URL(preview);
  if (
    upstream.protocol !== 'http:' ||
    !['127.0.0.1', 'localhost'].includes(upstream.hostname) ||
    !upstream.port ||
    upstream.username ||
    upstream.password ||
    upstream.pathname !== '/' ||
    upstream.search ||
    upstream.hash
  )
    throw new Error('Logout fixture requires a configured loopback preview origin.');
  let signedOut = false;
  const requests: { path: string; status: number; document?: string }[] = [];
  const server = createServer((request, response) => {
    const target = request.url;
    if (!target?.startsWith('/') || target.startsWith('//') || target.includes('\\')) {
      response.writeHead(400).end();
      return;
    }
    const url = new URL(target, upstream);
    if (url.origin !== upstream.origin || url.pathname !== target.split('?')[0] || url.hash) {
      response.writeHead(400).end();
      return;
    }
    if (url.pathname === '/auth/logout') {
      signedOut = logoutStatus === 303;
      requests.push({ path: '/auth/logout', status: logoutStatus });
      if (signedOut) response.setHeader('Location', '/');
      response.writeHead(logoutStatus).end();
    } else if (url.pathname === '/auth/session') {
      const status = signedOut ? 401 : 200;
      requests.push({ path: '/auth/session', status });
      response
        .writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' })
        .end(
          JSON.stringify(signedOut ? { authenticated: false, silent_sign_in: false } : signedIn),
        );
    } else if (url.pathname === '/' && signedOut && landingStatus === 503) {
      requests.push({ path: '/', status: 503, document: request.headers['sec-fetch-dest'] });
      response.writeHead(503).end('Synthetic landing unavailable');
    } else {
      const fail = () => {
        if (!response.headersSent) response.writeHead(502).end();
        else response.destroy();
      };
      const forward = get(
        {
          hostname: upstream.hostname,
          port: upstream.port,
          path: url.pathname + url.search,
        },
        (asset) => {
          const status = asset.statusCode ?? 502;
          if (status >= 300 && status < 400) {
            asset.resume();
            fail();
            return;
          }
          requests.push({
            path: url.pathname,
            status,
            document: request.headers['sec-fetch-dest'],
          });
          response.writeHead(status, {
            'Content-Type': asset.headers['content-type'] ?? 'application/octet-stream',
          });
          asset.on('error', fail);
          asset.pipe(response);
        },
      );
      forward.on('error', fail);
      forward.setTimeout(8000, () => forward.destroy());
      response.on('close', () => forward.destroy());
    }
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Missing local logout fixture port');
  return {
    origin: `http://127.0.0.1:${address.port}`,
    signOutSequence: () =>
      requests
        .slice(requests.findIndex(({ path }) => path === '/auth/logout'))
        .filter(({ path }) => ['/', '/auth/logout', '/auth/session'].includes(path)),
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      }),
  };
}

test('LOCAL logout fixture restricts preview forwarding to the configured origin', async ({
  baseURL,
}) => {
  const fixture = await logoutServer(baseURL!, 303, 200);
  try {
    const expected = await fetch(new URL('/build-info.json', baseURL));
    const forwarded = await fetch(new URL('/build-info.json', fixture.origin));
    expect(forwarded.status).toBe(200);
    expect(await forwarded.text()).toBe(await expected.text());

    // Only canonical, origin-relative HTTP request targets are accepted.
    for (const path of ['http://example.invalid/build-info.json', '/assets/../build-info.json']) {
      const status = await new Promise<number | undefined>((resolve, reject) => {
        get({ hostname: '127.0.0.1', port: new URL(fixture.origin).port, path }, (response) => {
          response.resume();
          resolve(response.statusCode);
        }).on('error', reject);
      });
      expect(status).toBe(400);
    }
  } finally {
    await fixture.close();
  }
});

async function prepare(page: Page, origin: string) {
  const boundary = new LocalStartup();
  await boundary.route(page, origin);
  await page.route(`${origin}/auth/session`, (route) => route.continue());
  return boundary;
}

const composer = (page: Page) => page.getByRole('textbox', { name: 'Message', exact: true });
async function signOut(page: Page) {
  await page.getByRole('button', { name: /Account menu for/ }).click();
  await page.getByRole('menuitem', { name: 'Sign out', exact: true }).click();
}

for (const [name, logoutStatus, landingStatus] of [
  ['ordinary logout', 303, 200],
  ['landing unavailable', 303, 503],
  ['revocation unavailable', 503, 200],
] as const) {
  test(`LOCAL sign-out HTTP boundary: ${name}`, async ({ page, context, baseURL }) => {
    const server = await logoutServer(baseURL!, logoutStatus, landingStatus);
    const sibling = await context.newPage();
    try {
      await Promise.all([page, sibling].map((tab) => prepare(tab, server.origin)));
      for (const tab of [page, sibling]) await tab.goto(`${server.origin}/agents/agt_contract`);
      await composer(sibling).fill('Synthetic private draft');
      await signOut(page);
      if (logoutStatus === 503) {
        await expect(page.getByText('Sign-out did not complete. Please try again.')).toBeVisible();
        await expect(composer(sibling)).toHaveValue('Synthetic private draft');
        expect(await sibling.evaluate((key) => localStorage.getItem(key), noticeKey)).toBeNull();
        expect(server.signOutSequence()).toEqual([{ path: '/auth/logout', status: 503 }]);
        return;
      }
      await expect(composer(sibling)).toHaveCount(0);
      await expect(page).toHaveURL(`${server.origin}/`);
      await expect
        .poll(() => server.signOutSequence().slice(0, 3))
        .toEqual([
          { path: '/auth/logout', status: 303 },
          { path: '/auth/session', status: 401 },
          { path: '/', status: landingStatus, document: 'document' },
        ]);
    } finally {
      await server.close();
    }
  });
}

test('LOCAL storage failure still signs out live tabs and masks restored tabs until reverified', async ({
  page,
  context,
  baseURL,
}) => {
  const server = await logoutServer(baseURL!, 303, 200);
  const sibling = await context.newPage();
  const reader = await context.newPage();
  const restored = await context.newPage();
  let release!: () => void;
  const held = new Promise<void>((resolve) => (release = resolve));
  let holdSession = false;
  let identityStarted = false;
  let readStarted = false;
  let restoreCheckStarted = false;
  let releaseRestore!: () => void;
  const heldRestore = new Promise<void>((resolve) => (releaseRestore = resolve));
  try {
    await Promise.all([page, sibling, reader, restored].map((tab) => prepare(tab, server.origin)));
    await page.addInitScript((key) => {
      const setItem = Storage.prototype.setItem;
      Storage.prototype.setItem = function (name, value) {
        if (this === localStorage && name === key) throw new DOMException('', 'QuotaExceededError');
        return setItem.call(this, name, value);
      };
    }, noticeKey);
    await restored.addInitScript((key) => {
      const getItem = Storage.prototype.getItem;
      Storage.prototype.getItem = function (name) {
        if (this === localStorage && name === key) throw new DOMException('', 'SecurityError');
        return getItem.call(this, name);
      };
      Object.defineProperty(window, 'BroadcastChannel', { value: undefined });
    }, noticeKey);
    await sibling.route(`${server.origin}/auth/session`, async (route) => {
      if (!holdSession) return route.continue();
      holdSession = false;
      identityStarted = true;
      await held;
      return route.fulfill({ json: signedIn });
    });
    await reader.route(
      (url) => url.pathname === '/api/v1/sandboxes',
      async (route) => {
        readStarted = true;
        await held;
        return route.fulfill({ json: { items: [] } });
      },
    );
    let holdRestore = false;
    await restored.route(`${server.origin}/auth/session`, async (route) => {
      if (holdRestore) {
        restoreCheckStarted = true;
        await heldRestore;
      }
      return route.continue();
    });
    for (const tab of [page, sibling, restored, reader])
      await tab.goto(`${server.origin}/agents/agt_contract`);
    await composer(sibling).fill('Synthetic private draft');
    await expect(composer(restored)).toBeVisible();
    await expect.poll(() => readStarted).toBe(true);
    const readCancelled = reader.waitForEvent('requestfailed', {
      predicate: (request) => new URL(request.url()).pathname === '/api/v1/sandboxes',
    });
    holdSession = true;
    await sibling.evaluate(() => window.dispatchEvent(new Event('online')));
    await expect.poll(() => identityStarted).toBe(true);
    const lateIdentity = sibling.waitForEvent('requestfinished', {
      predicate: (request) => new URL(request.url()).pathname === '/auth/session',
    });
    await signOut(page);
    await expect(composer(sibling)).toHaveCount(0);
    await expect(reader.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await readCancelled;
    expect(await sibling.evaluate((key) => localStorage.getItem(key), noticeKey)).toBeNull();
    release();
    await lateIdentity;
    await expect(composer(sibling)).toHaveCount(0);
    await expect(composer(restored)).toBeVisible();

    holdRestore = true;
    await restored.evaluate(() =>
      window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true })),
    );
    await expect.poll(() => restoreCheckStarted).toBe(true);
    await expect(composer(restored)).toHaveCount(0);
    releaseRestore();
    await expect(restored.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  } finally {
    release();
    releaseRestore();
    await server.close();
  }
});
