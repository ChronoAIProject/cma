import { object, strings, text, type Fields } from './model';

export function telegramMemberJoinGroupIds(source: Fields): string[] {
  const admission = object(source.admission);
  const chats = object(admission.chats);
  return admission.type === 'scoped' &&
    object(admission.senders).type === 'open' &&
    chats.type === 'allowlist' &&
    admission.groups === 'all'
    ? [...new Set(strings(chats.ids).filter((id) => /^-[1-9][0-9]*$/.test(id)))]
    : [];
}

export function telegramRelayJoinIdentityReady(source: Fields): boolean {
  return (
    source.platform === 'telegram' &&
    /^[1-9][0-9]{0,19}$/.test(text(source.bot_id)) &&
    strings(source.route_ids).some((id) => id.trim() !== '') &&
    source.trust_normalized !== true
  );
}
