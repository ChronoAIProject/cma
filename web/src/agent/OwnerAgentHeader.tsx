import { useState, type ComponentProps } from 'react';
import type { AgentDetail } from '../api/types';
import { useCmaClient } from '../api/context';
import { AgentShareLinksDialog } from '../resources/AgentShareLinks';
import { Modal } from '../ui/Modal';
import { SkillsSettings } from './SkillsSettings';
import { AgentHeader } from './AgentHeader';
import { AgentPreferencesDialog } from './AgentPreferencesDialog';
import { usePromptAvailability } from './usePromptAvailability';
import { readRetainedPromptIntent } from './transport/journal';

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
  const [menuOpen, setMenuOpen] = useState(false);
  const canInspectPrompt = () => {
    if (readOnly || !header.viewerId || !client.readAgentPreferences) return false;
    try {
      return !!readRetainedPromptIntent(sessionStorage, header.viewerId, detail.id);
    } catch {
      return false;
    }
  };
  const canReviewPrompt = menuOpen && canInspectPrompt();
  const promptReason = usePromptAvailability(
    client,
    detail.id,
    header.viewerId,
    menuOpen &&
      !readOnly &&
      !configBusy &&
      !!client.readAgentPreferences &&
      !!client.updateAgentPreferences,
  );
  const [dialog, setDialog] = useState<'share' | 'skills' | 'rename' | 'prompt'>();
  return (
    <>
      <AgentHeader
        {...header}
        onActionsOpenChange={setMenuOpen}
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
              (!canReviewPrompt &&
                (!client.updateAgentPreferences || configBusy || promptReason !== undefined)),
            reason: readOnly
              ? 'Only the task owner can update its system prompt.'
              : canReviewPrompt
                ? 'Review the pending system prompt change. New changes remain blocked until it is resolved.'
                : !client.readAgentPreferences || !client.updateAgentPreferences
                  ? 'Task preferences are unsupported by this client.'
                  : configBusy
                    ? 'Wait until the Agent and its pending actions are idle.'
                    : promptReason,
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
          else if (id === 'update-prompt') {
            if (!readOnly && (canInspectPrompt() || (!configBusy && promptReason === undefined)))
              setDialog('prompt');
          } else if (id === 'update-skills') setDialog('skills');
          else header.onMenuSelect?.(id);
        }}
      />
      {readOnly ? null : (
        <>
          {dialog === 'rename' || dialog === 'prompt' ? (
            <AgentPreferencesDialog
              agentId={detail.id}
              viewerId={header.viewerId}
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
              ownerId={header.viewerId}
              agentId={detail.id}
              open={dialog === 'share'}
              onClose={() => setDialog(undefined)}
            />
          ) : null}
          <Modal
            open={dialog === 'skills'}
            title="Choose skills"
            description="Add skills to this Agent. Each installation uses the version you choose."
            onClose={() => setDialog(undefined)}
          >
            {dialog === 'skills' ? (
              <SkillsSettings
                agentId={detail.id}
                viewerId={header.viewerId}
                active
                catalogOpen
                onClose={() => setDialog(undefined)}
                busy={configBusy}
              />
            ) : null}
          </Modal>
        </>
      )}
    </>
  );
}
