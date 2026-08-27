import { useCallback, useRef, useState } from 'react';
import { TransportError } from '../api/client';
import { newIntentKey } from './idempotency';

interface RetainedCommand {
  readonly key: string;
  readonly send: (key: string) => Promise<unknown>;
  inFlight: Promise<unknown> | null;
}

export interface PendingBrowserCommand {
  readonly slot: string;
  readonly label: string;
}

/**
 * Retain one browser command's identity and byte-equivalent send closure until
 * CMA returns a finite response. A second click joins an in-flight request and
 * an ambiguous retry always invokes the original closure, not newly rendered
 * component state.
 */
export class BrowserCommandLedger {
  readonly #commands = new Map<string, RetainedCommand>();

  async execute<T>(slot: string, send: (key: string) => Promise<T>): Promise<T> {
    const other = [...this.#commands.keys()].find((candidate) => candidate !== slot);
    if (other !== undefined) {
      throw new Error(`The ${other} command must be reconciled before starting another action.`);
    }

    let command = this.#commands.get(slot);
    if (command === undefined) {
      command = { key: newIntentKey(), send, inFlight: null };
      this.#commands.set(slot, command);
    }
    return this.#attempt<T>(slot, command);
  }

  async reconcile<T>(slot: string): Promise<T> {
    const command = this.#commands.get(slot);
    if (command === undefined) throw new Error(`No ${slot} command is awaiting reconciliation.`);
    return this.#attempt<T>(slot, command);
  }

  async #attempt<T>(slot: string, command: RetainedCommand): Promise<T> {
    if (command.inFlight !== null) return command.inFlight as Promise<T>;

    const attempt = command.send(command.key);
    command.inFlight = attempt;
    try {
      const result = await attempt;
      this.#commands.delete(slot);
      return result as T;
    } catch (cause) {
      if (!(cause instanceof TransportError)) this.#commands.delete(slot);
      throw cause;
    } finally {
      const retained = this.#commands.get(slot);
      if (retained === command) retained.inFlight = null;
    }
  }

  has(slot: string): boolean {
    return this.#commands.has(slot);
  }
}

interface IdempotentCommands {
  readonly pending: PendingBrowserCommand | null;
  readonly execute: <T>(
    slot: string,
    label: string,
    send: (key: string) => Promise<T>,
  ) => Promise<T>;
  readonly reconcile: <T>(slot: string, label: string) => Promise<T>;
}

/** One immediate same-key replay, then an explicit retained reconciliation state. */
export function useIdempotentCommand(): IdempotentCommands {
  const ledger = useRef(new BrowserCommandLedger());
  const pendingRef = useRef<PendingBrowserCommand | null>(null);
  const [pending, setPending] = useState<PendingBrowserCommand | null>(null);

  const run = useCallback(
    async <T>(slot: string, label: string, attempt: () => Promise<T>): Promise<T> => {
      const unresolved = pendingRef.current;
      if (unresolved !== null && unresolved.slot !== slot) {
        throw new Error(`${unresolved.label} must be reconciled before starting another action.`);
      }

      try {
        const result = await attempt();
        pendingRef.current = null;
        setPending(null);
        return result;
      } catch (cause) {
        if (!(cause instanceof TransportError)) {
          pendingRef.current = null;
          setPending(null);
          throw cause;
        }
      }

      const unresolvedCommand = { slot, label };
      pendingRef.current = unresolvedCommand;
      setPending(unresolvedCommand);
      try {
        const result = await attempt();
        pendingRef.current = null;
        setPending(null);
        return result;
      } catch (cause) {
        if (!(cause instanceof TransportError)) {
          pendingRef.current = null;
          setPending(null);
        }
        throw cause;
      }
    },
    [],
  );

  const execute = useCallback(
    <T>(slot: string, label: string, send: (key: string) => Promise<T>) =>
      run(slot, label, () => ledger.current.execute(slot, send)),
    [run],
  );
  const reconcile = useCallback(
    <T>(slot: string, label: string) => run(slot, label, () => ledger.current.reconcile<T>(slot)),
    [run],
  );

  return { pending, execute, reconcile };
}
