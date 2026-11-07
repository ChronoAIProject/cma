import { useState, type ComponentProps } from 'react';
import type { AgentDetail } from '../api/types';
import { useCmaClient } from '../api/context';
import { AgentShareLinksDialog } from '../resources/AgentShareLinks';
import { Modal } from '../ui/Modal';
import { SkillsSettings } from './SkillsSettings';
import { AgentHeader } from './AgentHeader';
import { AgentPreferencesDialog } from './AgentPreferencesDialog';

/** Owner-only tools reuse the same public authorization as their Details controls. */
export function OwnerAgentHeader({
  detail,
  readOnly,
  busy,
  actionsBusy = false,
  onTitleUpdated,
  ...header
}: Omit<ComponentProps<typeof AgentHeader>, 'sandbox' | 'onShare'> & {
  readonly detail: AgentDetail;
  readonly readOnly: boolean;
  readonly busy: boolean;
  readonly actionsBusy?: boolean;
  readonly onTitleUpdated?: (title: string) => void;
}) {
  const client = useCmaClient();
  const configBusy = busy || actionsBusy;
  const [dialog, setDialog] = useState<'share' | 'skills' | 'rename' | 'prompt'>();
  return (
    <>
      <AgentHeader
        {...header}
        onShare={readOnly || !client.resourceViews ? undefined : () => setDialog('share')}
        menu={[
          {
            id: 'rename-agent',
            label: 'Rename',
            disabled:
              readOnly ||
              !client.readAgentPreferences ||
              !client.updateAgentPreferences ||
              actionsBusy,
            reason: readOnly
              ? 'Only the task owner can rename it.'
              : !client.readAgentPreferences || !client.updateAgentPreferences
                ? 'Task preferences are unsupported by this client.'
                : actionsBusy
                  ? 'Another action is still being confirmed.'
                  : undefined,
          },
          {
            id: 'update-prompt',
            label: 'Update system prompt',
            disabled:
              readOnly ||
              !client.readAgentPreferences ||
              !client.updateAgentPreferences ||
              configBusy,
            reason: readOnly
              ? 'Only the task owner can update its system prompt.'
              : !client.readAgentPreferences || !client.updateAgentPreferences
                ? 'Task preferences are unsupported by this client.'
                : configBusy
                  ? 'Wait until the Agent and its pending actions are idle.'
                  : undefined,
          },
          {
            id: 'update-skills',
            label: 'Update skills',
            disabled:
              readOnly || !client.readAgentSkills || !client.installAgentSkill || configBusy,
            reason: readOnly
              ? 'Only the task owner can update its skills.'
              : !client.readAgentSkills || !client.installAgentSkill
                ? 'Agent skills are unsupported by this client.'
                : configBusy
                  ? 'Wait until the Agent and its pending actions are idle.'
                  : undefined,
          },
          ...(header.menu ?? []),
        ]}
        onMenuSelect={(id) => {
          if (id === 'rename-agent') setDialog('rename');
          else if (id === 'update-prompt') setDialog('prompt');
          else if (id === 'update-skills') setDialog('skills');
          else header.onMenuSelect?.(id);
        }}
      />
      {readOnly ? null : (
        <>
          {dialog === 'rename' || dialog === 'prompt' ? (
            <AgentPreferencesDialog
              agentId={detail.id}
              mode={dialog}
              currentTitle={header.title}
              active={dialog === 'prompt' && configBusy}
              onTitleUpdated={onTitleUpdated}
              onClose={() => setDialog(undefined)}
            />
          ) : null}
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
            {dialog === 'skills' ? (
              <SkillsSettings agentId={detail.id} active busy={configBusy} />
            ) : null}
          </Modal>
        </>
      )}
    </>
  );
}
