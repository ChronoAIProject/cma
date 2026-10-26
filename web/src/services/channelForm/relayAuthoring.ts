import {
  modeUnavailable,
  nativeDiscordUnavailable,
  type RelayDiscovery,
} from '../RelayCapabilities';
import { credentialsPatch, object, records, type Fields } from './model';

function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) =>
    item !== null && typeof item === 'object' && !Array.isArray(item)
      ? Object.fromEntries(Object.entries(item).sort(([a], [b]) => a.localeCompare(b)))
      : item,
  );
}
/** Block new unsupported selections, never rewrite saved records or uncertain retries. */
export function validateRelayAuthoring(
  discovery: RelayDiscovery,
  next: Fields,
  previous?: Fields,
  credentials: Fields = {},
) {
  const oldSources = records(previous?.sources);
  for (const source of records(next.sources)) {
    const unchanged = oldSources.some((old) => canonical(old) === canonical(source));
    if (source.type === 'discord') {
      const hasSavedNative = oldSources.some((old) => old.type === 'discord');
      const reason = nativeDiscordUnavailable(discovery, !hasSavedNative);
      if (
        reason &&
        (!unchanged || Boolean(credentialsPatch(credentials, false)?.discord_bot_token))
      )
        throw new Error(
          `${reason} Keep saved Discord settings unchanged. Your draft has been retained.`,
        );
      if (!hasSavedNative && !credentialsPatch(credentials, false)?.discord_bot_token)
        throw new Error('Enter the Discord bot token. Saved token values are never displayed.');
      continue;
    }
    if (source.type !== 'nyxid_relay') continue;
    const platform = String(source.platform);
    const mode =
      source.verify_raw_event === true
        ? 'raw_event'
        : source.trust_normalized === true
          ? 'trusted_normalized'
          : ['telegram', 'lark', 'feishu'].includes(platform)
            ? 'native_event'
            : undefined;
    if (!mode && !unchanged)
      throw new Error(`Choose an explicit incoming message verification policy for ${platform}.`);
    if (!mode) continue;
    const reason = modeUnavailable(discovery, platform, mode);
    if (!reason) continue;
    const sourceChanged =
      !unchanged ||
      (mode === 'raw_event' &&
        (canonical(next.sources) !== canonical(previous?.sources) ||
          canonical(next.partition) !== canonical(previous?.partition) ||
          Object.keys(credentialsPatch(credentials, false) ?? {}).some(
            (key) => key !== 'agent_key',
          )));
    if (sourceChanged)
      throw new Error(
        `${reason} Keep the saved source settings unchanged, or ask the gateway operator to enable this mode. Your draft has been retained.`,
      );
  }
  // Exact existing thread contracts remain editable elsewhere when writes are disabled.
  const thread = object(next.reply).thread_contract;
  if (
    thread &&
    discovery.authoring?.thread_contract_writes_enabled === false &&
    canonical(thread) !== canonical(object(previous?.reply).thread_contract)
  ) {
    throw new Error(
      'New or changed thread-following settings are disabled on this gateway. Your draft has been retained.',
    );
  }
}
