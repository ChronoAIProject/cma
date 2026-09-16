# Connect a personal Telegram bot

Use a dedicated NyxID bot and Agent key owned by the same account as your
personal CMA Bridge. Keep the CMA setup tab open while configuring NyxID so
the prepared callback remains attached to the same setup.

1. Open [New bridge](https://bot.chrono-ai.fun/bridges/new). Under **Connect your
   NyxID key**, copy the callback address. The bridge has not been created yet.
2. In NyxID, open [Agent keys](https://nyx.chrono-ai.fun/keys) and create a
   dedicated Agent key if needed. Open its detail page and set **Callback URL**
   to the exact CMA address. Keep the key available for the later import.
3. Open [Channel Bots](https://nyx.chrono-ai.fun/channel-bots), select your
   Telegram bot, and choose **Add Route**. Select the prepared Agent key. For a
   private conversation, use its actual chat ID and **Private** chat type.
   A default route is appropriate only for a dedicated bot: changing its
   default can deactivate a previous default route.
4. Return to CMA and select **Refresh connection choices**. Choose that bot,
   key and route. For a separate conversation environment, choose **A separate
   Sandbox for each conversation**, **Each sender in each chat**, and published
   Sandbox, Workspace and Agent profile revisions.
5. Restrict senders and chats to **Only selected IDs**, enter your actual
   Telegram IDs, and keep **Refuse group messages**. Review resource ownership,
   context sharing and reply visibility, then choose **Save disabled bridge**.
6. Import the selected NyxID Agent key on the saved bridge. Explicitly select
   **Allow this bridge to use my NyxID connection for 30 days** to authorize
   background connection checks and creation. Choose **Validate connection**,
   then **Enable bridge** only after validation succeeds.

**Let the Agent choose when to reply** allows the Agent’s explicit channel reply
tool. Ordinary final text does not automatically send a Telegram message. A
channel-created Agent receives `read_channel_context` and `reply_to_channel`;
manual and Trigger launches do not gain channel reply authority from that setup.
The separate-Sandbox choice does not need retained-Agent context consent.

If the browser loses the result of saving, **Recover saved bridge** retries the
original saved request. Reloading or returning in the same tab preserves the
prepared callback and the recovery request. Recover before changing the setup;
do not create a replacement callback or route to work around a pending save.

## Deployment prerequisites

The deployment must provide NyxID setup and its public HTTPS callback origin,
published compatible profiles, renewable creation authority and scoped key
provisioning. The Agent runtime must advertise HTTP MCP support in its actual
ACP handshake (`agentCapabilities.mcpCapabilities.http: true`) so the channel
tools can be attached. Ordinary conversation continuity alone does not verify
that support.

These six deployment switches enable the minimal Telegram path. Operators
must apply them to the intended deployment; this guide does not apply settings.

```json
{
  "CHRONO_BRIDGE_MANAGEMENT_ENABLED": "true",
  "CHRONO_BRIDGE_INGRESS_ENABLED": "true",
  "CHRONO_BRIDGE_ALLOCATION_ENABLED": "true",
  "CHRONO_BRIDGE_EXECUTION_ENABLED": "true",
  "CHRONO_BRIDGE_OUTBOUND_ENABLED": "true",
  "CHRONO_BRIDGE_TELEGRAM_ENABLED": "true"
}
```

The switches are global. Personal scope comes from your owned connection,
personal Bridge, selected sender/chat IDs and group refusal. Other platforms,
media, source controls, shared resource reuse, Trigger and optional UI switches
are not needed for this setup.
