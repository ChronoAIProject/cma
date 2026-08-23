import { PlatformSettingsConflict } from './types';
import type {
  ActivityEntry,
  ActivityQuery,
  AdminSession,
  PlatformSettings,
  SessionBriefing,
  StoredPlatformSettings,
  OperatorStore,
  Person,
  RuntimeInventory,
} from './types';

/**
 * Stand-in operator surface.
 *
 * Exists so the panel can be driven without a server: components consume the
 * port, so a test can hand them this and exercise every state — including the
 * ones a real deployment produces rarely, like a provider that cannot be asked
 * what it is running.
 *
 * It narrows by actor the way the control plane does rather than handing the
 * caller everything and trusting the component to filter. The guarantee under
 * test is that the person filter reaches the server; a fake that ignored it
 * would let a regression pass.
 *
 * Nothing here is written to storage, and no fixture carries anything that
 * looks like a credential.
 */

export interface MockOperatorOptions {
  readonly people?: readonly Person[];
  readonly activity?: readonly ActivityEntry[];
  readonly sessions?: readonly AdminSession[];
  readonly inventory?: RuntimeInventory;
  readonly platformSettings?: StoredPlatformSettings;
  /** Briefing restored by reset. Defaults to the mock's shipped briefing. */
  readonly shippedBriefing?: SessionBriefing;
  /** Force every call to fail, so error paths can be exercised. */
  readonly failWith?: string;
  /** Told which session was stopped, and which was ended. */
  readonly onStop?: (id: string) => void;
  readonly onTerminate?: (id: string) => void;
  readonly onSettingsChange?: (settings: PlatformSettings) => void;
  readonly onBriefingReset?: () => void;
}

const NOTHING_RUNNING: RuntimeInventory = {
  running: 0,
  fullyObserved: true,
  providers: [],
};

/** A complete settings document for local and component-level operator work. */
export const MOCK_PLATFORM_SETTINGS: StoredPlatformSettings = {
  revision: 0,
  settings: {
    briefing: {
      before: [
        '# Working in this session',
        'This session is already authenticated. Do not run `nyxid login`.',
      ],
      reachHeading: '## What you can reach',
      reachLead: 'Use only the service slugs granted below.',
      reachClose: 'Calls to every other service are refused.',
      reachNone: 'This session was granted no services.',
      after: ['Continue with the work the owner requested.'],
    },
    sizeBounds: {
      minCpuMillis: 250,
      maxCpuMillis: 4000,
      minMemoryMib: 256,
      maxMemoryMib: 8192,
    },
    liveSessions: null,
  },
};

function copyBriefing(briefing: SessionBriefing): SessionBriefing {
  return { ...briefing, before: [...briefing.before], after: [...briefing.after] };
}

function copySettings(settings: PlatformSettings): PlatformSettings {
  return {
    briefing: copyBriefing(settings.briefing),
    sizeBounds: { ...settings.sizeBounds },
    liveSessions: settings.liveSessions,
  };
}

function copyStored(stored: StoredPlatformSettings): StoredPlatformSettings {
  return {
    revision: stored.revision,
    settings: copySettings(stored.settings),
    previous: stored.previous === undefined ? undefined : copySettings(stored.previous),
  };
}

export function createMockOperatorStore(options: MockOperatorOptions = {}): OperatorStore {
  const sessions = [...(options.sessions ?? [])];
  let platform = copyStored(options.platformSettings ?? MOCK_PLATFORM_SETTINGS);
  const shippedBriefing = copyBriefing(
    options.shippedBriefing ?? MOCK_PLATFORM_SETTINGS.settings.briefing,
  );

  function guard(): void {
    if (options.failWith !== undefined) {
      throw new Error(options.failWith);
    }
  }

  /** Move a session to the state the control plane would leave it in. */
  function moveTo(id: string, status: string): void {
    const index = sessions.findIndex((s) => s.id === id);
    const existing = sessions[index];
    if (existing === undefined) {
      throw new Error('That session no longer exists.');
    }
    sessions[index] = { ...existing, status };
  }

  return {
    async people() {
      guard();
      return options.people ?? [];
    },

    async activity(query: ActivityQuery) {
      guard();
      const entries = options.activity ?? [];
      const narrowed =
        query.actor === undefined ? entries : entries.filter((e) => e.actor === query.actor);
      return query.limit === undefined ? narrowed : narrowed.slice(0, query.limit);
    },

    async runtimes() {
      guard();
      return options.inventory ?? NOTHING_RUNNING;
    },

    async sessions() {
      guard();
      return [...sessions];
    },

    async stopSession(id: string) {
      guard();
      // Mirrors the control plane: stopping releases the runtime and leaves the
      // session, so the row stays and only its state changes.
      moveTo(id, 'idle');
      options.onStop?.(id);
    },

    async terminateSession(id: string) {
      guard();
      moveTo(id, 'terminated');
      options.onTerminate?.(id);
    },

    async readPlatformSettings() {
      guard();
      return copyStored(platform);
    },

    async changePlatformSettings(expectedRevision: number, settings: PlatformSettings) {
      guard();
      if (expectedRevision !== platform.revision) {
        throw new PlatformSettingsConflict('Platform settings changed after they were read.');
      }
      const previous = copySettings(platform.settings);
      platform = {
        revision: platform.revision + 1,
        settings: copySettings(settings),
        previous,
      };
      options.onSettingsChange?.(copySettings(settings));
      return copyStored(platform);
    },

    async resetPlatformBriefing(expectedRevision: number) {
      guard();
      if (expectedRevision !== platform.revision) {
        throw new PlatformSettingsConflict('Platform settings changed after they were read.');
      }
      const previous = copySettings(platform.settings);
      platform = {
        revision: platform.revision + 1,
        settings: {
          ...copySettings(platform.settings),
          briefing: copyBriefing(shippedBriefing),
        },
        previous,
      };
      options.onBriefingReset?.();
      return copyStored(platform);
    },
  };
}
