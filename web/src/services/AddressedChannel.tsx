import { useId, useState } from 'react';
import { can, type Denied, type ServiceSession } from './client';
import { ChannelEditor, DeleteChannel, type Channel } from './ChannelEditor';
import { ChannelMap } from './ChannelMap';
import { NodeInspector } from './NodeInspector';
import { ChannelNodeTests } from './ChannelNodeTests';
import { ChannelOutboundMessage } from './ChannelOutboundMessage';
import { channelNodes, selectionKey, type ChannelSelection } from './channelTopology';
import { ChannelActivityView } from './ChannelActivity';
import { ChannelActivityPreview } from './ChannelActivityPreview';
import { useChannelActivity } from './useChannelActivity';
export function AddressedChannel({
  channel,
  active,
  writable,
  deletable,
  session,
  denied,
  onChanged,
  onDeleted,
}: {
  readonly channel: Channel;
  readonly active: boolean;
  readonly writable: boolean;
  readonly deletable: boolean;
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly onChanged: (channel?: Channel) => void;
  readonly onDeleted: () => void;
}) {
  const inspectorId = useId();
  const [inspecting, setInspecting] = useState(false);
  const [sendAvailable, setSendAvailable] = useState(false);
  const [inspectorSection, setInspectorSection] = useState<'overview' | 'activity'>('overview');
  const [selection, setSelection] = useState<ChannelSelection>({ kind: 'channel' });
  const [editorSelection, setEditorSelection] = useState<
    Exclude<ChannelSelection, { kind: 'toolsets' }>
  >({ kind: 'channel' });
  const current = channel;
  const activity = useChannelActivity(current.channel_id, session.subject, active);
  const nodes = channelNodes(current);
  const editable = can(session, 'event-gateway', 'channels:update');
  const newProducer =
    editable && selection.kind === 'producer' && selection.index === nodes.producers.length;
  const exists =
    selection.kind === 'channel' ||
    selection.kind === 'toolsets' ||
    newProducer ||
    [...nodes.producers, ...nodes.consumers].some((node) => node.key === selectionKey(selection));
  const selected: ChannelSelection = exists ? selection : { kind: 'channel' };
  return (
    <div className="channel-workspace">
      <ChannelMap
        channel={current}
        toolsets={nodes.toolsets}
        selection={selected}
        activityPreview={<ChannelActivityPreview feed={activity} />}
        onActivity={() => {
          setSelection({ kind: 'channel' });
          setEditorSelection({ kind: 'channel' });
          setInspectorSection('activity');
          setInspecting(true);
        }}
        onSelect={(next) => {
          setInspectorSection('overview');
          setSelection(next);
          if (next.kind !== 'toolsets') setEditorSelection(next);
          setInspecting(true);
        }}
        inspectorId={inspectorId}
        canAddProducer={editable && writable}
      />
      <NodeInspector
        channel={current}
        selection={selected}
        id={inspectorId}
        open={active && inspecting}
        initialSection={inspectorSection}
        sendAvailable={sendAvailable}
        testsAvailable={!newProducer}
        onClose={() => setInspecting(false)}
        send={
          editable ? (
            <ChannelOutboundMessage
              writable={writable}
              owner={session.subject}
              onAvailability={setSendAvailable}
              channel={current}
              selection={selected}
              denied={denied}
            />
          ) : undefined
        }
        tests={
          can(session, 'event-gateway', 'channels:test') ? (
            <div hidden={newProducer}>
              <ChannelNodeTests
                writable={writable}
                channelId={current.channel_id}
                channelVersion={current.version}
                selection={selected}
                denied={denied}
              />
            </div>
          ) : undefined
        }
        activity={selected.kind === 'channel' ? <ChannelActivityView feed={activity} /> : undefined}
        danger={
          selected.kind === 'channel' && can(session, 'event-gateway', 'channels:delete') ? (
            <DeleteChannel
              allowed={deletable}
              channel={current}
              denied={denied}
              onDone={onDeleted}
            />
          ) : undefined
        }
      >
        {editable ? (
          <ChannelEditor
            writable={writable}
            channel={current}
            section={selected.kind === 'toolsets' ? editorSelection : selected}
            denied={denied}
            onDone={(updated) => {
              onChanged(updated);
            }}
          />
        ) : (
          <p className="channel-field-hint">You have read-only access to this configuration.</p>
        )}
      </NodeInspector>
    </div>
  );
}
