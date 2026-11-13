import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as current from './desktopPreparation';
import * as retained from '../test/compatibility/desktopPreparation.release';
import { desktopSocket, stubDesktopSockets } from './desktopPreparation.fixture';

const sourceDirectory = dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(
  readFileSync(
    resolve(
      sourceDirectory,
      '../../../api/crates/layer1-public/tests/fixtures/retained-clients/manifest.json',
    ),
    'utf8',
  ),
) as {
  desktop_client: { sha256: string; previous_commit: string; current_commit: string };
};

it('executes the exact retained module shared by the two deployed Web revisions', () => {
  const bytes = readFileSync(
    resolve(sourceDirectory, '../test/compatibility/desktopPreparation.release.ts'),
  );
  expect(createHash('sha256').update(bytes).digest('hex')).toBe(manifest.desktop_client.sha256);
  expect(manifest.desktop_client.previous_commit).toBe('f779e5310595fb62e19537a76f664ba52e8342f9');
  expect(manifest.desktop_client.current_commit).toBe('4f03c35a80322673ec00df7a3b6b9399015f5a68');
});

beforeEach(() => {
  vi.useFakeTimers();
  stubDesktopSockets();
  vi.stubGlobal('location', { protocol: 'https:', host: 'contract.example' });
  vi.stubGlobal(
    'fetch',
    vi.fn(() => {
      throw new Error('Network forbidden in compatibility fixtures');
    }),
  );
});
afterEach(() => {
  vi.runAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

for (const [name, client] of [
  ['previous', retained],
  ['released-current', retained],
  ['develop', current],
] as const) {
  describe(`${name} desktop client contract`, () => {
    it('uses the retained discovery route, encodes the Agent and selects ws/wss', () => {
      const corpus = JSON.parse(
        readFileSync(
          resolve(
            sourceDirectory,
            '../../../api/crates/layer1-public/tests/fixtures/retained-clients/previous-desktop.json',
          ),
          'utf8',
        ),
      ) as {
        cases: { id: string; response: { body: { websocket_url: string } } }[];
      };
      const path = corpus.cases.find(({ id }) => id === 'desktop-discovery')!.response.body
        .websocket_url;
      expect(client.desktopSocketUrl('agt_contract')).toBe(`wss://contract.example${path}`);
      expect(client.desktopSocketUrl('agt/one')).toBe(
        'wss://contract.example/api/v1/agents/agt%2Fone/desktop/ws',
      );
      vi.stubGlobal('location', { protocol: 'http:', host: 'contract.example' });
      expect(client.desktopSocketUrl('agt_contract')).toBe(`ws://contract.example${path}`);
    });

    it('accepts the binary RFB greeting and transfers it once in order with no input', () => {
      const scope = { context: {}, owner: 'fixture-owner' };
      const prep = client.prepareDesktop(scope.context, 'agt_contract', scope.owner)!;
      const socket = desktopSocket();
      const frames = [
        new Uint8Array([82, 70, 66, 32]).buffer,
        new Uint8Array([48, 48, 51, 46, 48, 48, 56, 10]).buffer,
      ];
      frames.forEach((frame) => socket.receive(frame));
      expect(socket.binaryType).toBe('arraybuffer');
      expect(socket.send).not.toHaveBeenCalled();
      expect(client.takePreparedDesktop(scope, 'agt_contract')).toBeUndefined();
      prep.handoff();
      prep.release();
      const taken = client.takePreparedDesktop(scope, 'agt_contract')!;
      const received: unknown[] = [];
      socket.onmessage = (event) => received.push(event.data);
      taken.replay();
      taken.replay();
      expect(received).toEqual(frames);
      expect(client.takePreparedDesktop(scope, 'agt_contract')).toBeUndefined();
      expect(socket.close).not.toHaveBeenCalled();
      expect(fetch).not.toHaveBeenCalled();
    });

    it.each(['wrong-owner', 'wrong-agent', 'text-frame', 'oversize', 'expired', 'transport-error'])(
      'refuses incompatible or stale handoff: %s',
      (fault) => {
        const scope = { context: {}, owner: 'fixture-owner' };
        const prep = client.prepareDesktop(scope.context, 'agt_contract', scope.owner)!;
        const socket = desktopSocket();
        prep.handoff();
        if (fault === 'text-frame') socket.receive('RFB 003.008\n');
        if (fault === 'oversize') socket.receive(new ArrayBuffer(65_537));
        if (fault === 'expired') vi.advanceTimersByTime(10_000);
        if (fault === 'transport-error') socket.fail();
        expect(
          client.takePreparedDesktop(
            { ...scope, owner: fault === 'wrong-owner' ? 'another-fixture' : scope.owner },
            fault === 'wrong-agent' ? 'agt_other' : 'agt_contract',
          ),
        ).toBeUndefined();
        expect(socket.close).toHaveBeenCalledOnce();
        expect(socket.send).not.toHaveBeenCalled();
        expect(fetch).not.toHaveBeenCalled();
      },
    );
  });
}
