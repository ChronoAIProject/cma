import type { ReactNode } from 'react';
import type { ProfileReference } from '../../api/types';
import type { ProfileKind } from '../../routing/routes';
import type { ResolvedProfileName, ResolvedProfileRevision } from './client';
import { ReferenceLink } from '../ReferenceLink';

function Fact({ label, children }: { readonly label: string; readonly children: ReactNode }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

function Commands({ commands }: { readonly commands: readonly string[] }) {
  return commands.length === 0 ? (
    <p>None</p>
  ) : (
    <ol className="marketplace-commands">
      {commands.map((command, index) => (
        <li key={index}>
          <pre>{command}</pre>
        </li>
      ))}
    </ol>
  );
}

function References({
  kind,
  references,
  names,
}: {
  readonly kind: ProfileKind;
  readonly references: readonly ProfileReference[];
  readonly names: readonly ResolvedProfileName[];
}) {
  return references.length === 0 ? (
    <span>None</span>
  ) : (
    <ol>
      {references.map((reference, index) => (
        <li key={`${reference.id}:${String(index)}`}>
          <ReferenceLink
            kind={kind}
            id={reference.id}
            name={
              names.find(
                (entry) =>
                  entry.kind === kind &&
                  entry.id === reference.id &&
                  entry.revision === reference.revision,
              )?.name
            }
          />{' '}
          · Revision {reference.revision}
        </li>
      ))}
    </ol>
  );
}

const EFFORT = {
  minimal: 'Minimal',
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  x_high: 'Very high',
};

export function RevisionContents({
  value,
  names,
  compact = false,
}: {
  readonly value: ResolvedProfileRevision;
  readonly names: readonly ResolvedProfileName[];
  readonly compact?: boolean;
}) {
  if (value.kind === 'agent') {
    const content = value.revision.content;
    return (
      <dl className="marketplace-facts" data-compact={compact || undefined}>
        <Fact label="Model">{content.model ?? 'Deployment default'}</Fact>
        <Fact label="Thinking effort">{EFFORT[content.thinkingEffort]}</Fact>
        <Fact label="How freely it acts">
          {content.permissionLevel === 'guarded' ? 'Guarded' : 'Autonomous'}
        </Fact>
        <Fact label="Skills">
          {content.skills.length === 0 ? (
            'None'
          ) : (
            <ul>
              {content.skills.map((skill, index) => (
                <li key={index}>
                  <ReferenceLink kind="skill" id={skill.id} version={skill.version} /> ·{' '}
                  {skill.version ?? 'Latest available version'}
                </li>
              ))}
            </ul>
          )}
        </Fact>
        <Fact label="System prompt">
          <pre>{content.systemPrompt || 'None'}</pre>
        </Fact>
      </dl>
    );
  }
  if (value.kind === 'workspace') {
    const content = value.revision.content;
    return (
      <dl className="marketplace-facts" data-compact={compact || undefined}>
        <Fact label="Location">
          {content.location.type !== 'directory' ? 'GitHub repository' : 'Empty directory'}
        </Fact>
        {content.location.type !== 'directory' ? (
          <>
            <Fact label="Repository URL">{content.location.url}</Fact>
            <Fact label="Branch or tag">{content.location.reference ?? 'Default branch'}</Fact>
          </>
        ) : (
          <Fact label="Empty directory name">{content.location.name}</Fact>
        )}
        <Fact label="Directory name">{content.name || 'Derived from repository name'}</Fact>
        <Fact label="Setup commands">
          <Commands commands={content.setupCommands} />
        </Fact>
        <Fact label="Agent profiles">
          <References kind="agent" references={content.agentProfiles} names={names} />
        </Fact>
      </dl>
    );
  }
  const content = value.revision.content;
  return (
    <dl className="marketplace-facts" data-compact={compact || undefined}>
      <Fact label="Size">
        {content.cpuMillis / 1000} cores · {content.memoryMib / 1024} GiB
      </Fact>
      <Fact label="Model access">
        {content.modelAccess === 'chronoai_managed' ? 'ChronoAI managed' : 'Your own account'}
      </Fact>
      <Fact label="Services it can reach">
        {content.nyxidServices.length === 0 ? (
          'None'
        ) : (
          <ul>
            {content.nyxidServices.map((service, index) => (
              <li key={index}>
                <ReferenceLink kind="service" id={service} />
              </li>
            ))}
          </ul>
        )}
      </Fact>
      <Fact label="Maximum workspaces">{content.maxWorkspaces}</Fact>
      <Fact label="Maximum agents per workspace">{content.maxAgentsPerWorkspace}</Fact>
      <Fact label="Workspace profiles">
        <References kind="workspace" references={content.workspaceProfiles} names={names} />
      </Fact>
      <Fact label="Software to install">
        <Commands commands={content.softwareInstallCommands} />
      </Fact>
      <Fact label="Environment variables">
        {content.environmentVariables.length === 0 ? (
          'None'
        ) : (
          <dl className="marketplace-environment">
            {content.environmentVariables.map((entry, index) => (
              <div key={index}>
                <dt>
                  <code>{entry.name}</code>
                </dt>
                <dd>
                  <pre>{entry.value}</pre>
                </dd>
              </div>
            ))}
          </dl>
        )}
      </Fact>
    </dl>
  );
}
