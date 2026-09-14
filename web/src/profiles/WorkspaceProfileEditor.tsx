import { useRef, useState } from 'react';
import type {
  ProfileDocument,
  ProfileDraft,
  ProfileEditorClient,
  ProfilesClient,
  RepositoryResolver,
  WorkspaceProfileContent,
} from './client';
import type { MarketplaceClient } from './marketplace/client';
import { ProfileEditor } from './ProfileEditor';
import { CommandList } from './pickers/CommandList';
import { ProfileReferencePicker } from './pickers/ProfileReferencePicker';
import { WorkspaceLocationPicker } from './pickers/WorkspaceLocationPicker';
import { repositoryKey, validateWorkspaceProfile } from './workspaceValidation';

const INITIAL: ProfileDraft<WorkspaceProfileContent> = {
  name: '',
  description: '',
  content: {
    location: { type: 'git_repository', url: '' },
    name: '',
    setupCommands: [],
    agentProfiles: [],
  },
};

export interface WorkspaceProfileEditorProps {
  readonly client: ProfileEditorClient<WorkspaceProfileContent>;
  readonly profiles: ProfilesClient;
  readonly marketplace: Pick<MarketplaceClient, 'listMarketplace'>;
  readonly resolver?: RepositoryResolver;
  readonly profileId?: string;
  readonly onSaved?: (profile: ProfileDocument<WorkspaceProfileContent>) => void;
}

export function WorkspaceProfileEditor(props: WorkspaceProfileEditorProps) {
  return <WorkspaceEditor key={props.profileId ?? 'new'} {...props} />;
}

function WorkspaceEditor({
  client,
  profiles,
  marketplace,
  resolver,
  profileId,
  onSaved,
}: WorkspaceProfileEditorProps) {
  const [verified, setVerified] = useState<string>();
  const verifiedForValidation = useRef<string | undefined>(undefined);
  return (
    <ProfileEditor
      client={client}
      profileId={profileId}
      initial={INITIAL}
      label="Workspace"
      onSaved={onSaved}
      validate={(draft) => {
        const errors = { ...validateWorkspaceProfile(draft) };
        if (
          draft.content.location.type === 'git_repository' &&
          verifiedForValidation.current !== repositoryKey(draft.content.location)
        )
          errors.repositoryUrl ??= 'Verify the repository and reference before saving.';
        return errors;
      }}
    >
      {(editor) => {
        const content = (next: Partial<WorkspaceProfileContent>) =>
          editor.change({ ...editor.draft, content: { ...editor.draft.content, ...next } });
        return (
          <>
            <section className="profile-editor__band">
              <h2>Location</h2>
              <WorkspaceLocationPicker
                editor={editor}
                resolver={resolver}
                verified={verified}
                onVerified={(key) => {
                  verifiedForValidation.current = key;
                  setVerified(key);
                }}
              />
            </section>
            <section className="profile-editor__band">
              <h2>Setup commands</h2>
              <CommandList
                value={editor.draft.content.setupCommands}
                onChange={(setupCommands) => content({ setupCommands })}
                label="Setup command"
                help="These commands run after the repository is ready, as the sandbox user."
                maximum={16}
                disabled={editor.locked}
                errors={editor.errors}
                field="setupCommands"
              />
            </section>
            <section className="profile-editor__band">
              <h2>Agent profiles</h2>
              <ProfileReferencePicker
                kind="agent"
                profiles={profiles}
                marketplace={marketplace}
                value={editor.draft.content.agentProfiles}
                onChange={(agentProfiles) => content({ agentProfiles })}
                maximum={8}
                disabled={editor.locked}
                error={editor.errors.agentProfiles}
              />
            </section>
          </>
        );
      }}
    </ProfileEditor>
  );
}
