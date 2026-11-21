import { Input } from '../ui/Input';
import type {
  CapacityBounds,
  CapacityBoundsClient,
  ProfileDocument,
  ProfileDraft,
  ProfileEditorClient,
  ProfilesClient,
  SandboxProfileContent,
  ServiceCatalogClient,
} from './client';
import type { MarketplaceClient } from './marketplace/client';
import { ChoiceGroup } from './ChoiceGroup';
import { Field } from './Field';
import { fieldProps, ProfileEditor } from './ProfileEditor';
import { CommandList } from './pickers/CommandList';
import { EnvironmentEditor } from './pickers/EnvironmentEditor';
import { ProfileReferencePicker } from './pickers/ProfileReferencePicker';
import { ResourceFields } from './pickers/ResourceFields';
import { ServicePicker } from './pickers/ServicePicker';
import { validateSandboxProfile } from './sandboxValidation';
import { useCapacityBounds } from './capacityBounds';
import { SandboxAvailabilityFields } from './SandboxAvailabilityFields';
import { isCodexConnection } from './modelProvider';

const INITIAL: ProfileDraft<SandboxProfileContent> = {
  name: '',
  description: '',
  content: {
    cpuMillis: 2000,
    memoryMib: 4096,
    nyxidServices: [],
    agentProvider: 'codex',
    modelAccess: 'chronoai_managed',
    maxWorkspaces: 4,
    maxAgentsPerWorkspace: 4,
    workspaceProfiles: [],
    softwareInstallCommands: [],
    environmentVariables: [],
  },
};

export interface SandboxProfileEditorProps {
  readonly client: ProfileEditorClient<SandboxProfileContent>;
  readonly profiles: ProfilesClient;
  readonly marketplace: Pick<MarketplaceClient, 'listMarketplace'>;
  readonly services?: ServiceCatalogClient;
  readonly bounds?: CapacityBounds;
  readonly boundsClient?: CapacityBoundsClient;
  readonly profileId?: string;
  readonly onSaved?: (profile: ProfileDocument<SandboxProfileContent>) => void;
}

export function SandboxProfileEditor(props: SandboxProfileEditorProps) {
  return <SandboxEditor key={props.profileId ?? 'new'} {...props} />;
}

function SandboxEditor({
  client,
  profiles,
  marketplace,
  services,
  bounds: providedBounds,
  boundsClient,
  profileId,
  onSaved,
}: SandboxProfileEditorProps) {
  const capacity = useCapacityBounds(providedBounds === undefined ? boundsClient : undefined);
  const bounds = providedBounds ?? capacity.bounds;
  return (
    <ProfileEditor
      client={client}
      profileId={profileId}
      initial={INITIAL}
      label="Sandbox"
      onSaved={onSaved}
      validate={(draft) => validateSandboxProfile(draft, bounds)}
    >
      {(editor) => {
        const value = editor.draft.content;
        const content = (next: Partial<SandboxProfileContent>) =>
          editor.change({ ...editor.draft, content: { ...value, ...next } });
        return (
          <>
            <SandboxAvailabilityFields editor={editor} />
            <section className="profile-editor__band">
              <h2>Size</h2>
              <ResourceFields
                editor={editor}
                bounds={bounds}
                loading={capacity.loading}
                onRetry={capacity.failed ? capacity.retry : undefined}
              />
            </section>
            <section className="profile-editor__band">
              <h2>NyxID services</h2>
              <ServicePicker
                catalog={services}
                value={value.nyxidServices.filter((service) => !isCodexConnection(service))}
                onChange={(nyxidServices) => content({ nyxidServices })}
                disabled={editor.locked}
              />
            </section>
            <section className="profile-editor__band">
              <h2>Model access</h2>
              <div className="profile-field">
                <ChoiceGroup
                  label="Model access"
                  value={value.modelAccess}
                  options={[
                    { value: 'chronoai_managed', label: 'Chrono LLM (public)' },
                    { value: 'own_credentials', label: 'Your ChatGPT subscription' },
                  ]}
                  disabled={editor.locked}
                  describedBy="profile-model-access-help"
                  onChange={(modelAccess) =>
                    content({
                      modelAccess,
                      nyxidServices: value.nyxidServices.filter(
                        (service) => !isCodexConnection(service),
                      ),
                    })
                  }
                />
                <p id="profile-model-access-help" className="profile-field__help">
                  {value.modelAccess === 'chronoai_managed'
                    ? 'The Agent uses Chrono LLM (public) through NyxID. No personal subscription is needed.'
                    : 'Each user signs in with their own ChatGPT subscription through OpenAI before their sandbox opens.'}
                </p>
              </div>
            </section>
            <section className="profile-editor__band">
              <h2>Limits</h2>
              <div className="profile-field-pair">
                <Field
                  id="profile-maxWorkspaces"
                  label="Maximum workspaces"
                  help="Allow between 1 and 16 workspaces in each sandbox."
                  error={editor.errors.maxWorkspaces}
                >
                  <Input
                    {...fieldProps(editor, 'maxWorkspaces')}
                    type="number"
                    min={1}
                    max={16}
                    step={1}
                    value={Number.isFinite(value.maxWorkspaces) ? value.maxWorkspaces : ''}
                    onChange={(event) => content({ maxWorkspaces: event.target.valueAsNumber })}
                  />
                </Field>
                <Field
                  id="profile-maxAgentsPerWorkspace"
                  label="Maximum agents per workspace"
                  help="Allow between 1 and 16 agents in each workspace."
                  error={editor.errors.maxAgentsPerWorkspace}
                >
                  <Input
                    {...fieldProps(editor, 'maxAgentsPerWorkspace')}
                    type="number"
                    min={1}
                    max={16}
                    step={1}
                    value={
                      Number.isFinite(value.maxAgentsPerWorkspace)
                        ? value.maxAgentsPerWorkspace
                        : ''
                    }
                    onChange={(event) =>
                      content({ maxAgentsPerWorkspace: event.target.valueAsNumber })
                    }
                  />
                </Field>
              </div>
            </section>
            <section className="profile-editor__band">
              <h2>Workspace profiles</h2>
              <ProfileReferencePicker
                kind="workspace"
                profiles={profiles}
                marketplace={marketplace}
                value={value.workspaceProfiles}
                onChange={(workspaceProfiles) => content({ workspaceProfiles })}
                maximum={
                  Number.isInteger(value.maxWorkspaces)
                    ? Math.min(16, Math.max(0, value.maxWorkspaces))
                    : 0
                }
                disabled={editor.locked}
                error={editor.errors.workspaceProfiles}
              />
            </section>
            <section className="profile-editor__band">
              <h2>Software to install</h2>
              <CommandList
                value={value.softwareInstallCommands}
                onChange={(softwareInstallCommands) => content({ softwareInstallCommands })}
                label="Install command"
                help="This runs once, before any credential exists; installs must live under /home/cma."
                maximum={32}
                disabled={editor.locked}
                errors={editor.errors}
                field="softwareInstallCommands"
              />
            </section>
            <section className="profile-editor__band">
              <h2>Environment variables</h2>
              <EnvironmentEditor editor={editor} />
            </section>
          </>
        );
      }}
    </ProfileEditor>
  );
}
