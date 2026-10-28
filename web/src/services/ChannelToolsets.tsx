import type { Channel } from './ChannelEditor';
import { channelNodes, consumerConfig, list, object } from './channelTopology';
import { Section } from './channelForm/Fields';
import { useRelayCapabilities, type AgentToolCapability } from './RelayCapabilities';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';

const labels: Readonly<Record<string, string>> = {
  readEventContext: 'Read the incoming event',
  replyToEvent: 'Reply to the sender',
  updateMessage: 'Edit a sent message',
  sendTyping: 'Show a typing indicator',
  readEventAttachment: 'Read received files',
  readMessageCapabilities: 'Check message actions',
  deleteMessage: 'Delete a sent message',
  setMessageReaction: 'React to a message',
  listEventThreads: 'Browse conversation threads',
  createEventThread: 'Start a thread from a message',
  manageThread: 'Manage a conversation thread',
  readEventHistory: 'Read earlier conversation messages',
  prepareFormReply: 'Prepare a WhatsApp form',
  readFormPreparation: 'Check form preparation and delivery',
};
/** Known source prerequisites from the public tool descriptions; never an execution grant. */
function missingSetup(tool: string, channel: Channel): string | undefined {
  if (tool === 'readEventHistory') {
    const sources = channel.definition.sources
      .map(object)
      .filter((s) => s.type === 'discord' || s.type === 'native_lark');
    if (sources.some((s) => s.conversation_history === true)) return undefined;
    if (sources.length === 0) return 'Requires a direct Discord or Lark / Feishu source';
    const discord = sources.some((s) => s.type === 'discord');
    const lark = sources.some((s) => s.type === 'native_lark');
    return discord && lark
      ? 'Enable Discord or Lark / Feishu conversation history'
      : discord
        ? 'Enable Discord conversation history'
        : 'Enable Lark / Feishu conversation history';
  }
  const requirements: Readonly<Record<string, readonly [string, string, string]>> = {
    listEventThreads: ['discord', 'thread_lifecycle', 'Enable Discord thread management'],
    createEventThread: ['discord', 'thread_lifecycle', 'Enable Discord thread management'],
    manageThread: ['discord', 'thread_lifecycle', 'Enable Discord thread management'],
    prepareFormReply: ['whatsapp', 'forms_enabled', 'Enable WhatsApp Forms'],
  };
  const requirement = Object.hasOwn(requirements, tool) ? requirements[tool] : undefined;
  if (!requirement) return undefined;
  const [type, consent, hint] = requirement;
  const sources = channel.definition.sources.map(object).filter((source) => source.type === type);
  if (sources.length === 0)
    return type === 'discord'
      ? 'Requires a direct Discord source'
      : 'Requires a direct WhatsApp source';
  return sources.some((source) => source[consent] === true) ? undefined : hint;
}
function Tools({
  items,
  channel,
}: {
  readonly items: readonly AgentToolCapability[];
  readonly channel: Channel;
}) {
  return (
    <ul className="channel-tools-list">
      {items.map((tool) => (
        <Presence key={tool.name}>
          <li>
            <div>
              {Object.hasOwn(labels, tool.name) ? <strong>{labels[tool.name]}</strong> : null}
              <code>{tool.name}</code>
            </div>
            <p>{tool.description}</p>
            {missingSetup(tool.name, channel) ? (
              <p className="channel-field-hint">{missingSetup(tool.name, channel)}</p>
            ) : null}
            <p>
              {tool.read_only ? 'Read only' : 'Can make changes'}
              {tool.destructive ? ' · May remove or replace data' : ''}
            </p>
          </li>
        </Presence>
      ))}
    </ul>
  );
}
/** Read the shared catalog without inferring channel permissions or agent installation. */
export function ChannelToolsets({ channel }: { readonly channel: Channel }) {
  const discovery = useRelayCapabilities();
  const { consumers } = channelNodes(channel);
  const conditional =
    discovery.agentTools?.filter((tool) => !missingSetup(tool.name, channel)) ?? [];
  const other = discovery.agentTools?.filter((tool) => missingSetup(tool.name, channel)) ?? [];
  return (
    <>
      <Section
        title="Event tools"
        description="Available tools depend on the event, platform and permissions."
      >
        {discovery.agentTools?.length ? (
          <>
            {conditional.length > 0 ? <Tools items={conditional} channel={channel} /> : null}
            {other.length > 0 ? (
              <details>
                <summary>Tools requiring setup ({other.length})</summary>
                <p className="channel-field-hint">These tools need additional source settings.</p>
                <Tools items={other} channel={channel} />
              </details>
            ) : null}
          </>
        ) : (
          <p className="channel-field-hint" role="status">
            {discovery.agentTools
              ? 'This gateway advertises no agent tools.'
              : discovery.status === 'loading'
                ? 'Checking available agent tools…'
                : discovery.status === 'failed'
                  ? 'Agent tool availability could not be loaded.'
                  : 'Tool availability is not reported by this gateway.'}
            {discovery.status === 'failed' && discovery.retry ? (
              <Button onClick={discovery.retry}>Retry capability check</Button>
            ) : null}
          </p>
        )}
      </Section>
      <Section title="Connected agents" description="Each connected agent has its own tool setup.">
        <ul className="channel-tools-list">
          {consumers.map((node) => {
            const config = consumerConfig(
              channel,
              node.selection.kind === 'consumer' ? node.selection.id : 'default',
            );
            const existing = object(object(config.provider).target).type === 'existing_agent';
            const extra = list(object(object(config.profile).context).mcp_servers).length;
            return (
              <li key={node.key}>
                <div>
                  <strong>{node.title}</strong>
                  <span>{node.subtitle}</span>
                </div>
                <p>
                  {node.pending
                    ? 'Connection pending.'
                    : existing
                      ? 'Existing agent. Uses its current tool setup.'
                      : 'Gateway tools requested for this agent.'}
                </p>
                {extra > 0 ? (
                  <p>
                    {extra} additional MCP {extra === 1 ? 'server is' : 'servers are'} requested in
                    this agent’s settings.
                  </p>
                ) : null}
              </li>
            );
          })}
        </ul>
      </Section>
    </>
  );
}
