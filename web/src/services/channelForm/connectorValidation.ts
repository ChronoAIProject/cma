import { object, records, strings, text, type Fields } from './model';

export const missingLarkAllowedSender = 'Add at least one allowed sender ID.';

export function botIdMessage(value: unknown, platform: 'telegram' | 'discord') {
  const id = text(value);
  const maximum = platform === 'telegram' ? '9223372036854775807' : '18446744073709551615';
  return /^[1-9][0-9]*$/.test(id) &&
    (id.length < maximum.length || (id.length === maximum.length && id <= maximum))
    ? undefined
    : `Use a positive numeric bot ID without leading zeros, up to ${maximum}.`;
}
export function telegramUsernameMessage(value: unknown) {
  return !text(value) || /^[A-Za-z0-9_]{5,32}$/.test(text(value))
    ? undefined
    : 'Use 5–32 letters, numbers or underscores, without @.';
}
export function larkIdentityMessage(value: unknown, key: string) {
  const prefix = key === 'app_id' ? 'cli_' : key === 'bot_open_id' ? 'ou_' : '';
  return /^[A-Za-z0-9_-]{1,128}$/.test(text(value)) && text(value).startsWith(prefix)
    ? undefined
    : `Use 1–128 letters, numbers, underscores or hyphens${prefix ? `, starting with ${prefix}` : ''}.`;
}
export function validateConnectorFields(draft: Fields) {
  records(draft.sources).forEach((source, index) => {
    const check = (path: string, message: string | undefined) => {
      if (message) throw new Error(`/sources/${index}/${path}: ${message}`);
    };
    if (source.type === 'telegram' || source.type === 'discord') {
      check('bot_id', botIdMessage(source.bot_id, source.type));
      if (source.type === 'telegram')
        check('bot_username', telegramUsernameMessage(source.bot_username));
    }
    if (source.type === 'native_lark') {
      const identity = object(source.identity);
      for (const key of ['app_id', 'tenant_key', 'bot_open_id'])
        check(`identity/${key}`, larkIdentityMessage(identity[key], key));
      const admission = object(source.admission);
      if (admission.type === 'allowlist') {
        const senders = strings(admission.sender_ids);
        if (!senders.length) check('admission', missingLarkAllowedSender);
        const blank = senders.findIndex((sender) => !sender.trim());
        if (blank !== -1) check(`admission/${blank}`, 'Enter a sender ID.');
      }
    }
  });
}
