import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import type { Page } from '@playwright/test';

const cases = JSON.parse(
  readFileSync(
    new URL(
      '../../api/crates/layer1-public/tests/fixtures/browser-auth-return-targets.json',
      import.meta.url,
    ),
    'utf8',
  ),
) as { target: string; backend: boolean }[];

/** Models redirects on loopback. Rust tests separately exercise CMA's login and callback. */
export class LocalSignIn {
  origin = '';
  authenticated = false;
  targets: string[] = [];
  callbacks = 0;
  errors: string[] = [];
  private appURL = '';
  private server = createServer((request, response) => {
    const url = new URL(request.url!, this.origin);
    const redirect = (location: string) => {
      response.writeHead(303, { location });
      response.end();
    };
    if (request.method === 'GET' && url.pathname === '/auth/login') {
      const target = url.searchParams.get('return_to') ?? '';
      this.targets.push(target);
      if (
        !url.searchParams.has('prompt') &&
        cases.some((item) => item.target === target && item.backend)
      )
        return redirect(`${this.origin}/provider`);
    } else if (request.method === 'GET' && url.pathname === '/provider') {
      response.writeHead(200, { 'content-type': 'text/html' });
      response.end('<a href="/callback?state=local-login">Complete local sign-in</a>');
      return;
    } else if (
      request.method === 'GET' &&
      url.pathname === '/callback' &&
      url.search === '?state=local-login' &&
      this.targets.length === 1
    ) {
      this.callbacks += 1;
      this.authenticated = true;
      return redirect(this.appURL + this.targets[0]);
    }
    this.errors.push(`${request.method} ${url.pathname}`);
    response.writeHead(400, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ title: 'Invalid local sign-in request' }));
  });

  async route(page: Page, baseURL: string) {
    this.appURL = baseURL;
    await new Promise<void>((resolve) => this.server.listen(0, '127.0.0.1', resolve));
    const address = this.server.address();
    if (!address || typeof address === 'string') throw new Error('Missing local sign-in port');
    this.origin = `http://127.0.0.1:${address.port}`;
    await page.route(`${this.origin}/**`, (route) => route.continue());
    await page.route(`${baseURL}/auth/login?**`, (route) => {
      const url = new URL(route.request().url());
      return route.continue({ url: this.origin + url.pathname + url.search });
    });
    await page.route(`${baseURL}/auth/session`, (route) =>
      this.authenticated
        ? route.fallback()
        : route.fulfill({
            status: 401,
            json: { authenticated: false, silent_sign_in: true },
          }),
    );
  }

  async close() {
    this.server.closeAllConnections();
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }
}
