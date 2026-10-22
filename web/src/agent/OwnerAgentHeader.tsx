import { useState, type ComponentProps } from 'react';
import type { AgentDetail } from '../api/types';
import { useCmaClient } from '../api/context';
import { AgentShareLinksDialog } from '../resources/AgentShareLinks';
import { Modal } from '../ui/Modal';
import { SkillsSettings } from './SkillsSettings';
import { AgentHeader } from './AgentHeader';
import { SandboxPopover } from './SandboxPopover';
import type { SandboxResourceAvailability } from './sandboxInput';

/** Owner-only tools reuse the same public authorization as their Details controls. */
export function OwnerAgentHeader({
  detail,
  availability,
  readOnly,
  busy,
  ...header
}: Omit<ComponentProps<typeof AgentHeader>, 'sandbox' | 'onShare'> & {
  readonly detail: AgentDetail;
  readonly availability: SandboxResourceAvailability;
  readonly readOnly: boolean;
  readonly busy: boolean;
}) {
  const client = useCmaClient();
  const [dialog, setDialog] = useState<'share' | 'skills'>();
  return (
    <>
      <AgentHeader
        {...header}
        sandbox={
          readOnly ? undefined : <SandboxPopover detail={detail} availability={availability} />
        }
        onShare={readOnly || !client.resourceViews ? undefined : () => setDialog('share')}
        menu={[
          ...(readOnly || !client.readAgentSkills
            ? []
            : [{ id: 'update-skills', label: 'Update skills installed' }]),
          ...(header.menu ?? []),
        ]}
        onMenuSelect={(id) => {
          if (id === 'update-skills') setDialog('skills');
          else header.onMenuSelect?.(id);
        }}
      />
      {readOnly ? null : (
        <>
          {client.resourceViews ? (
            <AgentShareLinksDialog
              client={client.resourceViews}
              agentId={detail.id}
              open={dialog === 'share'}
              onClose={() => setDialog(undefined)}
            />
          ) : null}
          <Modal
            open={dialog === 'skills'}
            title="Update skills installed"
            onClose={() => setDialog(undefined)}
          >
            {dialog === 'skills' ? <SkillsSettings agentId={detail.id} active busy={busy} /> : null}
          </Modal>
        </>
      )}
    </>
  );
}
