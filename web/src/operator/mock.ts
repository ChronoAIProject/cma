import type {
  ActivityEntry,
  ActivityQuery,
  AdminSession,
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
  /** Force every call to fail, so error paths can be exercised. */
  readonly failWith?: string;
  /** Told which session was stopped, and which was ended. */
  readonly onStop?: (id: string) => void;
  readonly onTerminate?: (id: string) => void;
}

const NOTHING_RUNNING: RuntimeInventory = {
  running: 0,
  fullyObserved: true,
  providers: [],
};

export function createMockOperatorStore(options: MockOperatorOptions = {}): OperatorStore {
  const sessions = [...(options.sessions ?? [])];

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
  };
}
