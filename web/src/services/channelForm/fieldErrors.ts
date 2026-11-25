const labels: Record<string, string> = {
  name: 'Channel name',
  issuer: 'NyxID issuer',
  key_id: 'Signing key ID',
  bot_id: 'Bot ID',
  bot_username: 'Bot username',
  app_id: 'App ID',
  tenant_key: 'Tenant key',
  bot_open_id: 'Bot open ID',
  separate_app: 'Separate webhook confirmation',
  ack_after_ms: 'Acknowledgement delay',
  thread_contract: 'Thread following',
};
export function fieldError(error: string) {
  const match = /^(\/[^\s:]*):\s*(.*)$/s.exec(error);
  if (!match) return { message: error };
  const path = match[1]!;
  const hmac = /^\/credentials\/hmac_secrets\/(\d+)\/(key_id|secret)$/.exec(path);
  const label = hmac
    ? `HMAC ${Number(hmac[1]) + 1} ${hmac[2] === 'secret' ? 'secret' : 'key ID'}`
    : /\/auth\/key_id$/.test(path)
      ? 'HMAC key ID'
      : (labels[path.split('/').at(-1)!] ?? 'Channel settings');
  return { path, message: match[2]!, label };
}
