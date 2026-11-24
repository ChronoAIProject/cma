/** Public NyxID setup remains in NyxID; CMA never fabricates owner credentials. */
export function RelaySetupGuidance({ platform }: { platform: string }) {
  const guidance: Record<string, string> = {
    telegram: 'Connect your Telegram bot in NyxID, then select a relay route for that bot.',
    lark: 'Connect your Lark app in NyxID and complete its event subscriptions and bot permissions before attaching its relay route.',
    feishu:
      'Connect your Feishu app in NyxID and complete its event subscriptions and bot permissions before attaching its relay route.',
    discord:
      'Connect your Discord application in NyxID and configure its interaction endpoint. Verify the upstream transport for ordinary channel messages separately; support here does not start a Discord Gateway connection.',
    whatsapp:
      'Connect your WhatsApp Cloud API business phone in NyxID and complete webhook verification and subscriptions before attaching its relay route.',
    slack:
      'Connect and configure your Slack relay in NyxID. This gateway requires an explicit normalized-message trust policy for Slack.',
  };
  return (
    <div className="channel-field-hint">
      <p>
        {guidance[platform] ??
          'Configure a relay route owned by you in NyxID before attaching it here.'}
      </p>
      <p>
        Use your NyxID route's exact issuer, signing key ID and route IDs, and enter its channel key
        under Credentials. Route IDs are not CMA connection references. After saving, configure the
        NyxID route to use this channel's NyxID callback URL. No bot or route is created by this
        form.
      </p>
    </div>
  );
}
