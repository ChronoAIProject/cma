import type { Channel } from './ChannelEditor';
import { channelNodes, consumerConfig, list, object } from './channelTopology';
import { Section } from './channelForm/Fields';

const core = [
  [
    'readEventContext',
    'Read the incoming event',
    'Read the sender, conversation and event details, including the delivery features available for this event.',
  ],
  [
    'replyToEvent',
    'Reply to the sender',
    'Send a response using the event’s configured reply destination.',
  ],
  [
    'updateMessage',
    'Edit a sent message',
    'Update a confirmed message when that platform and message support editing.',
  ],
  [
    'sendTyping',
    'Show a typing indicator',
    'Let the sender know the agent is working, where the delivery platform supports it.',
  ],
] as const;
const optional = [
  [
    'readEventAttachment',
    'Read received files',
    'Images, documents and transcripts from the event. Requires media support and a compatible agent binding.',
  ],
  [
    'readMessageCapabilities',
    'Check message actions',
    'Discover whether a confirmed message can be deleted or receive a reaction. Requires message effects to be enabled.',
  ],
  [
    'deleteMessage',
    'Delete a sent message',
    'Remove an eligible Telegram or Discord bot message. Platform permissions and age limits apply.',
  ],
  [
    'setMessageReaction',
    'React to a sent message',
    'Set or clear the bot’s reaction on an eligible Telegram or Discord message. Requires message effects.',
  ],
] as const;
function Tools({ items }: { readonly items: readonly (readonly [string, string, string])[] }) {
  return (
    <ul className="channel-tools-list">
      {items.map(([name, label, detail]) => (
        <li key={name}>
          <div>
            <strong>{label}</strong>
            <code>{name}</code>
          </div>
          <p>{detail}</p>
        </li>
      ))}
    </ul>
  );
}
/** Describe real gateway tools without claiming an event or agent supports every operation. */
export function ChannelToolsets({ channel }: { readonly channel: Channel }) {
  const { consumers } = channelNodes(channel);
  return (
    <>
      <Section
        title="Event tools"
        description="The gateway offers these tools to channel consumers. Each call needs the channel’s authorized agent key and a valid event or message reference."
      >
        <Tools items={core} />
      </Section>
      <Section
        title="Optional tools"
        description="These are discovered by the agent when enabled. This channel’s saved configuration alone does not confirm platform support or installation in an existing agent."
      >
        <Tools items={optional} />
      </Section>
      <Section
        title="Connected agents"
        description="Tool access follows each consumer’s binding. Selecting this node does not install tools or change permissions."
      >
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
                    ? 'Connection pending. Tool availability is not confirmed.'
                    : existing
                      ? 'Existing agent. Its current tool setup is preserved; this view does not confirm installation.'
                      : 'Gateway tool connection is supplied when this consumer’s agent is created.'}
                </p>
                {extra > 0 ? (
                  <p>
                    {extra} additional MCP {extra === 1 ? 'server is' : 'servers are'} configured in
                    its agent settings.
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
