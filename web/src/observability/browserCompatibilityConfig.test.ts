import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { BROWSER_COMPATIBILITY_COUNTERS } from './browserCompatibility';

const config = readFileSync(resolve(process.cwd(), 'nginx.conf'), 'utf8');

function counterMap(): Map<string, unknown> {
  const body = /map \$uri \$browser_compatibility_counter \{([\s\S]*?)\n\}/.exec(config)?.[1];
  expect(body, 'nginx.conf has no browser compatibility URI map').toBeDefined();

  const entries = new Map<string, unknown>();
  for (const match of (body ?? '').matchAll(/^\s*(\/\S+)\s+'([^']+)';$/gm)) {
    entries.set(match[1] ?? '', JSON.parse(match[2] ?? ''));
  }
  return entries;
}

function collectorLocation(): string {
  const start = config.indexOf('location ^~ /__bc/v1/ {');
  const end = config.indexOf('    # Hashed assets are immutable', start);
  expect(start, 'nginx.conf has no compatibility collector location').toBeGreaterThan(-1);
  expect(end, 'compatibility collector has no bounded end').toBeGreaterThan(start);
  return config.slice(start, end);
}

describe('browser compatibility Nginx collector', () => {
  it('has exactly the same literal allowlist as the browser observer', () => {
    const entries = counterMap();
    const clientPaths = BROWSER_COMPATIBILITY_COUNTERS.map(({ path }) => path).sort();

    expect([...entries.keys()].sort()).toEqual(clientPaths);
    for (const { path, tuple } of BROWSER_COMPATIBILITY_COUNTERS) {
      expect(entries.get(path)).toEqual({
        key: tuple[0],
        generation: tuple[1],
        outcome: tuple[2],
      });
    }
  });

  it('logs only the three bounded dimensions from fixed map values', () => {
    const entries = counterMap();
    const allowedKeys = new Set(['appearance']);
    const allowedGenerations = new Set(['canonical', 'historical-key']);
    const allowedOutcomes = new Set([
      'read-ok',
      'read-blocked',
      'write-ok',
      'write-blocked',
      'fallback-used',
    ]);

    for (const value of entries.values()) {
      expect(value).toEqual({
        key: expect.any(String),
        generation: expect.any(String),
        outcome: expect.any(String),
      });
      const dimensions = value as Record<string, string>;
      expect(allowedKeys.has(dimensions.key ?? '')).toBe(true);
      expect(allowedGenerations.has(dimensions.generation ?? '')).toBe(true);
      expect(allowedOutcomes.has(dimensions.outcome ?? '')).toBe(true);
    }

    const format = config.match(/^log_format browser_compatibility.*$/m)?.[0] ?? '';
    expect(format).toBe(
      "log_format browser_compatibility escape=none '$browser_compatibility_counter';",
    );
    for (const requestField of [
      '$remote_addr',
      '$request',
      '$request_uri',
      '$args',
      '$http_user_agent',
      '$http_referer',
      '$http_cookie',
      '$request_body',
    ]) {
      expect(format).not.toContain(requestField);
    }
  });

  it('accepts only empty, query-free POSTs and never proxies the collector', () => {
    const location = collectorLocation();

    expect(location).toContain('if ($browser_compatibility_counter = "")');
    expect(location).toContain('if ($request_method != POST)');
    expect(location).toContain('if ($args != "")');
    expect(location).toContain('if ($content_length !~ "^(0)?$")');
    expect(location).toContain('if ($http_transfer_encoding != "")');
    expect(location).toContain(
      'access_log /dev/stdout browser_compatibility if=$browser_compatibility_loggable;',
    );
    expect(location).toContain('add_header Cache-Control "no-store" always;');
    expect(location).toContain('return 204;');
    expect(location).not.toContain('proxy_pass');
    expect(location).not.toContain('$remote_addr');
    expect(location).not.toContain('$http_cookie');
    expect(location).not.toContain('$http_referer');
    expect(location).not.toContain('$http_user_agent');
  });
});
