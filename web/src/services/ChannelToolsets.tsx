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
function Tools({ items }: { readonly items: readonly AgentToolCapability[] }) {
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
  return (
    <>
      <Section
        title="Event tools"
        description="These tools are advertised by the gateway. Each call still checks authorization and platform support; this list does not confirm installation in an agent."
      >
        {discovery.agentTools?.length ? (
          <Tools items={discovery.agentTools} />
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
                      : 'A gateway tool connection is requested when this agent is created. Provider installation is not reported here.'}
                </p>
                {extra > 0 ? (
                  <p>
                    {extra} additional MCP {extra === 1 ? 'server is' : 'servers are'} requested in
                    its saved profile. Provider installation is not confirmed here.
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
