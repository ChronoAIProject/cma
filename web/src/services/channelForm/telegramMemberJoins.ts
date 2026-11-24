import { object, strings, type Fields } from './model';

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
