import { useState } from 'react';
import type { CreationIntent } from './BotCreate';

export function creationStorageKey(viewer: string) {
  return `cma:bot-creation:${JSON.stringify([location.origin, viewer])}`;
}

export function useCreationIntent(viewer: string) {
  const key = creationStorageKey(viewer);
  const [state, setState] = useState<{ intent?: CreationIntent; error?: string }>(() => {
    try {
      const raw = sessionStorage.getItem(key);
      if (raw === null) return {};
      const saved = JSON.parse(raw) as { version: number; intent: CreationIntent };
      const intent = saved.intent;
      if (
        saved.version !== 1 ||
        !intent ||
        typeof intent.key !== 'string' ||
        !/^[\x21-\x7e]{1,200}$/.test(intent.key) ||
        typeof intent.profileName !== 'string' ||
        typeof intent.botName !== 'string' ||
        typeof intent.body?.name !== 'string' ||
        (typeof intent.body.connection_ref !== 'string' &&
          typeof intent.body.nyxid_bot_id !== 'string') ||
        typeof intent.body.profile?.id !== 'string' ||
        !Number.isSafeInteger(intent.body.profile.revision) ||
        !intent.body.admission ||
        typeof intent.body.reply?.auto_final !== 'boolean' ||
        !Number.isSafeInteger(intent.body.reply.ack_after_ms)
      )
        throw new Error('Invalid saved Bot request');
      // The public SDK validates the full command again before dispatch.
      return { intent };
    } catch {
      return {
        error:
          'Saved Bot creation could not be read. Restore browser session storage before continuing.',
      };
    }
  });
  const retain = (intent: CreationIntent | undefined): boolean => {
    try {
      // Configuration references only; credentials never enter the browser.
      if (intent === undefined) sessionStorage.removeItem(key);
      else sessionStorage.setItem(key, JSON.stringify({ version: 1, intent }));
      setState({ intent });
      return true;
    } catch {
      setState((previous) => ({
        ...previous,
        error:
          'Bot creation could not be retained. Restore browser session storage before continuing.',
      }));
      return false;
    }
  };
  return { ...state, retain };
}
