import { useState } from 'react';
import type { CreationClient } from '../create/client';
import type { AgentProfileContent, ProfileEditorClient } from '../profiles/client';
import type { AppRoute } from '../routing/routes';
import { BotCreate, type CreationIntent } from './BotCreate';
import { BotFatherProfile } from './BotFatherProfile';
import { clearFatherDraft, readFatherDraft } from './fatherDraft';
import { clearFatherReceipt, fatherCreationProfile, readFatherReceipt } from './fatherReceipt';
import type { BotClient } from './model';

export function BotNew({
  client,
  profiles,
  agentProfiles,
  viewer,
  viewerLabel,
  intent,
  retain,
  onNavigate,
}: {
  readonly client: BotClient;
  readonly profiles: CreationClient;
  readonly agentProfiles?: ProfileEditorClient<AgentProfileContent>;
  readonly viewer: string;
  readonly viewerLabel: string;
  readonly intent?: CreationIntent;
  readonly retain: (value: CreationIntent | undefined) => boolean | void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [initial] = useState(() => {
    try {
      // An accepted owner-bound request takes precedence over another incoming suggestion.
      const receipt = readFatherReceipt(viewer);
      return { receipt, draft: receipt?.draft ?? readFatherDraft(), error: undefined };
    } catch {
      return {
        error: 'Bot Father setup could not be read. Check the setup link and browser storage.',
        receipt: undefined,
        draft: undefined,
      };
    }
  });
  const [receipt, setReceipt] = useState(initial.receipt);
  const prepared = receipt === undefined ? undefined : fatherCreationProfile(receipt);
  if (intent === undefined && initial.error)
    return (
      <section>
        <h1>Bot Father setup unavailable</h1>
        <p role="alert">{initial.error}</p>
      </section>
    );
  if (intent === undefined && initial.draft !== undefined && prepared === undefined)
    return agentProfiles === undefined ? (
      <section>
        <h1>Bot Father setup unavailable</h1>
        <p>Your account needs permission to create its own Agent Profiles.</p>
      </section>
    ) : (
      <BotFatherProfile
        client={agentProfiles}
        viewer={viewer}
        viewerLabel={viewerLabel}
        draft={initial.draft}
        receipt={receipt}
        onRetained={setReceipt}
      />
    );
  return (
    <BotCreate
      client={client}
      profiles={profiles}
      intent={intent}
      retain={retain}
      initialName={receipt?.draft.name}
      initialProfile={prepared}
      onNavigate={(route) => {
        if (route.kind === 'bot') {
          try {
            clearFatherReceipt(viewer);
            clearFatherDraft();
          } catch {
            // A confirmed Bot must remain successful even if optional draft cleanup fails.
          }
        }
        onNavigate(route);
      }}
    />
  );
}
