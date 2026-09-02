import { PlatformSettingsConflict } from './types';
import type {
  ActivityEntry,
  ActivityQuery,
  AdminSession,
  ChatHost,
  PlatformSettings,
  SessionBriefing,
  StoredPlatformSettings,
  OperatorStore,
  Person,
  RuntimeInventory,
  ChatSettings,
  StoredChatSettings,
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
  readonly chatHosts?: readonly ChatHost[];
  readonly inventory?: RuntimeInventory;
  readonly platformSettings?: StoredPlatformSettings;
  readonly chatSettings?: StoredChatSettings;
  /** Briefing restored when reset also removes Chat. Defaults to the shipped fixture. */
  readonly shippedBriefing?: SessionBriefing;
  /** Force every call to fail, so error paths can be exercised. */
  readonly failWith?: string;
  /** Told which session was stopped, and which was ended. */
  readonly onStop?: (id: string) => void;
  readonly onTerminate?: (id: string) => void;
  readonly onRetireHost?: (id: string) => void;
  readonly onSettingsChange?: (settings: PlatformSettings) => void;
  readonly onChatSettingsChange?: (settings: ChatSettings | null) => void;
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
    deploymentCapacity: {
      cpuMillis: null,
      memoryMib: null,
      assumedSessionSize: null,
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

export const MOCK_CHAT_SETTINGS: StoredChatSettings = {
  revision: 0,
  chat: {
    enabled: false,
    workspace: {
      repositoryUrl: 'https://github.com/the-omega-institute/trureturing',
      branch: 'dev',
    },
    agent: {
      model: null,
      reasoningEffort: 'high',
      instructions: '',
      recordToolBodies: true,
    },
    messagePrefixes: ['$codex-formal-answer'],
    inactivity: { pauseAfterSeconds: 900 },
    setup: { commands: [] },
    host: { size: null, maxThreads: 10 },
  },
};

function copyChat(settings: ChatSettings): ChatSettings {
  return {
    ...settings,
    workspace: { ...settings.workspace },
    agent: { ...settings.agent },
    messagePrefixes: [...settings.messagePrefixes],
    inactivity: { ...settings.inactivity },
    setup: { commands: [...settings.setup.commands] },
    host: {
      ...settings.host,
      size: settings.host.size === null ? null : { ...settings.host.size },
    },
  };
}

function copyStoredChat(stored: StoredChatSettings): StoredChatSettings {
  return {
    revision: stored.revision,
    chat: stored.chat === null ? null : copyChat(stored.chat),
    previousChat:
      stored.previousChat === undefined
        ? undefined
        : stored.previousChat === null
          ? null
          : copyChat(stored.previousChat),
  };
}

function copyBriefing(briefing: SessionBriefing): SessionBriefing {
  return { ...briefing, before: [...briefing.before], after: [...briefing.after] };
}

function copySettings(settings: PlatformSettings): PlatformSettings {
  return {
    briefing: copyBriefing(settings.briefing),
    deploymentCapacity: {
      ...settings.deploymentCapacity,
      assumedSessionSize:
        settings.deploymentCapacity.assumedSessionSize === null
          ? null
          : { ...settings.deploymentCapacity.assumedSessionSize },
    },
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
  const chatHosts = [...(options.chatHosts ?? [])];
  const initialPlatform = copyStored(options.platformSettings ?? MOCK_PLATFORM_SETTINGS);
  const initialChat = copyStoredChat(options.chatSettings ?? MOCK_CHAT_SETTINGS);
  const settingsRevision =
    options.platformSettings !== undefined ? initialPlatform.revision : initialChat.revision;
  let platform = { ...initialPlatform, revision: settingsRevision };
  let chat = { ...initialChat, revision: settingsRevision };
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

    async chatHosts() {
      guard();
      return chatHosts.map((host) => ({ ...host }));
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

    async retireChatHost(id: string) {
      guard();
      const index = chatHosts.findIndex((host) => host.id === id);
      const existing = chatHosts[index];
      if (existing === undefined) {
        throw new Error('That chat host no longer exists.');
      }
      chatHosts[index] = { ...existing, status: 'retired' };
      options.onRetireHost?.(id);
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
      const previousChat = chat.chat === null ? null : copyChat(chat.chat);
      const revision = platform.revision + 1;
      platform = {
        revision,
        settings: copySettings(settings),
        previous,
      };
      chat = {
        revision,
        chat: chat.chat === null ? null : copyChat(chat.chat),
        previousChat,
      };
      options.onSettingsChange?.(copySettings(settings));
      return copyStored(platform);
    },

    async resetPlatformSettings(expectedRevision: number) {
      guard();
      if (expectedRevision !== platform.revision) {
        throw new PlatformSettingsConflict('Platform settings changed after they were read.');
      }
      const previous = copySettings(platform.settings);
      const previousChat = chat.chat === null ? null : copyChat(chat.chat);
      const revision = platform.revision + 1;
      platform = {
        revision,
        settings: {
          ...copySettings(platform.settings),
          briefing: copyBriefing(shippedBriefing),
        },
        previous,
      };
      chat = { revision, chat: null, previousChat };
      options.onBriefingReset?.();
      options.onChatSettingsChange?.(null);
      return copyStored(platform);
    },

    async readChatSettings() {
      guard();
      return copyStoredChat(chat);
    },

    async changeChatSettings(expectedRevision: number, settings: ChatSettings | null) {
      guard();
      if (expectedRevision !== chat.revision) {
        throw new PlatformSettingsConflict('Chat settings changed after they were read.');
      }
      const previousChat = chat.chat === null ? null : copyChat(chat.chat);
      const previous = copySettings(platform.settings);
      const revision = chat.revision + 1;
      chat = {
        revision,
        chat: settings === null ? null : copyChat(settings),
        previousChat,
      };
      platform = {
        revision,
        settings: copySettings(platform.settings),
        previous,
      };
      options.onChatSettingsChange?.(settings === null ? null : copyChat(settings));
      return copyStoredChat(chat);
    },
  };
}
