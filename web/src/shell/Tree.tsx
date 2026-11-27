import { selectAgentAction } from './agentActionDispatch';
import { useTreeFocus } from './useTreeFocus';
import { Presence } from '../ui/Presence';
import { useId } from 'react';
import { useTreeCollapse } from './useTreeCollapse';
import {
  sandboxChildrenAvailable,
  sandboxChildrenReadable,
  sandboxChildStatus,
} from '../sandbox/lifecycle';
import { sandboxStatusPresentation } from '../sandbox/presentation';
import { sandboxHasParkedRuntimeUpdate } from '../sandbox/parkedRuntimeUpdate';
import { taskPresentation } from './taskPresentation';
import { toneDot } from '../ui/Badge';
import {
  IconBranchOutline16,
  IconChevronDownOutline14,
  IconChevronRightOutline14,
  IconDataOutline16,
  IconFolderOpenOutline16,
  IconPauseOutline16,
} from '../ui/icons';
import { TreeRow } from './TreeRow';
import { agentActions, sandboxActions, workspaceActions } from './treeActions';
import { CreationBadge, useTreeCreation } from './treeCreation';
import { statusLabel, statusTone } from '../vocabulary/status';
import { taskActivityOrder } from './taskGroups';
import {
  activityLabel,
  agentListAction,
  agentSignal,
  stateMeta,
  keyboardNavigation,
} from './treePresentation';
import { PersonalAgentTree } from './PersonalAgentTree';
import { DeletionProgressList } from './DeletionProgressList';
export { activityLabel } from './treePresentation';
import { useChannelFacts } from '../resources/useChannelFacts';
import { ChannelBadge, TreeFactsSummary, treeAgentIds } from './treeChannel';
import { agentDisplayTitle } from '../agent/presentation';
import { sandboxFilterEmpty, sandboxNavigation } from './navigationStatuses';

import type { TreeProps } from './treeTypes';
export type { TreeAction, TreePermissions, TreeProps } from './treeTypes';

export function Tree({
  currentTask,
  selectedRetired,
  onShowAll,
  attention,
  showDeletionProgress = true,
  active = true,
  resourceClient,
  viewerId,
  view = 'sandbox',
  statusFilter = 'all',
  items,
  refreshError,
  lifecycleBusy = false,
  busySandboxIds,
  deletingSandboxIds,
  onViewDeletion,
  route,
  permissions,
  onNavigate,
  onAgentIntent,
  onAction,
}: TreeProps) {
  const {
    groups,
    visible: visibleItems,
    deleting: deletingItems,
  } = sandboxNavigation(
    items,
    ({ sandbox }) => sandbox,
    view === 'agent' ? 'all' : statusFilter,
    deletingSandboxIds,
  );
  const facts = useTreeCreation(viewerId, visibleItems, active && refreshError === undefined);
  const channelIds = treeAgentIds(visibleItems);
  const channelFacts = useChannelFacts(
    resourceClient,
    channelIds,
    active && refreshError === undefined,
    viewerId,
  );
  const groupId = useId();
  const focus = useTreeFocus();
  const { collapsed, toggle } = useTreeCollapse(viewerId);

  return (
    <div className="resource-tree" {...focus}>
      {showDeletionProgress ? (
        <DeletionProgressList
          sandboxes={deletingItems.map(({ sandbox }) => sandbox)}
          onViewStatus={(id) =>
            onViewDeletion ? onViewDeletion(id) : onNavigate({ kind: 'sandbox', sandboxId: id })
          }
        />
      ) : null}
      <Presence>
        {refreshError === undefined ? null : (
          <p className="resource-tree__notice" role="alert">
            {refreshError}
          </p>
        )}
      </Presence>
      {view === 'agent' ? (
        <PersonalAgentTree
          viewerId={viewerId}
          currentTask={currentTask}
          selectedRetired={selectedRetired}
          onShowAll={onShowAll}
          attention={attention}
          active={active}
          items={visibleItems}
          statusFilter={statusFilter}
          route={route}
          permissions={permissions}
          lifecycleBusy={lifecycleBusy}
          busySandboxIds={busySandboxIds}
          refreshError={refreshError}
          onNavigate={onNavigate}
          onAgentIntent={onAgentIntent}
          onAction={onAction}
        />
      ) : (
        groups.map((group) => {
          const groupNode = `group:${group.id}`;
          const expanded = !collapsed.has(groupNode);
          return (
            <section
              className="resource-tree__section"
              key={group.id}
              aria-label={`${group.label} sandboxes`}
            >
              <button
                type="button"
                className="resource-tree__heading"
                aria-label={`${expanded ? 'Collapse' : 'Expand'} ${group.label} sandboxes`}
                aria-expanded={expanded}
                aria-controls={`${groupId}-${group.id}`}
                onClick={() => toggle(groupNode)}
              >
                {expanded ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
                <span>{group.label}</span>
                <span className="resource-tree__count">{group.items.length}</span>
              </button>
              <div id={`${groupId}-${group.id}`} hidden={!expanded}>
                {expanded ? (
                  <div
                    role="tree"
                    aria-label={`${group.label} sandboxes`}
                    onKeyDown={keyboardNavigation}
                  >
                    {group.items.map(({ sandbox, workspaces }) => {
                      const sandboxBusy = lifecycleBusy || busySandboxIds?.has(sandbox.id) === true;
                      const originalWorkspaces =
                        items.find((item) => item.sandbox.id === sandbox.id)?.workspaces ?? [];
                      const sandboxNode = `sandbox:${sandbox.id}`;
                      const childrenAvailable = sandboxChildrenReadable(sandbox);
                      const runtimeAvailable =
                        !sandboxBusy && !refreshError && sandboxChildrenAvailable(sandbox);
                      const sandboxExpanded = childrenAvailable && !collapsed.has(sandboxNode);
                      const presentation = sandboxStatusPresentation(sandbox);
                      const parked = sandboxHasParkedRuntimeUpdate(sandbox);
                      const paused = presentation.status === 'paused';
                      return (
                        <div className="resource-tree__branch" key={sandbox.id}>
                          <TreeRow
                            nodeId={sandboxNode}
                            level={1}
                            label={sandbox.name}
                            icon={<IconDataOutline16 />}
                            route={{ kind: 'sandbox', sandboxId: sandbox.id }}
                            selected={route.kind === 'sandbox' && route.sandboxId === sandbox.id}
                            expanded={childrenAvailable ? sandboxExpanded : undefined}
                            reviving={
                              presentation.status === 'reviving' &&
                              presentation.group === 'changing'
                            }
                            meta={
                              <>
                                <span className="tree-row__state" data-tone={presentation.tone}>
                                  {/* A stale status draws no dot: it may no longer be true. */}
                                  {refreshError === undefined ? toneDot(presentation.tone) : null}
                                  {presentation.label}
                                </span>
                                <TreeFactsSummary
                                  channelFacts={channelFacts}
                                  facts={facts}
                                  ids={originalWorkspaces
                                    .filter((workspace) => workspace.status !== 'removed')
                                    .flatMap((workspace) =>
                                      workspace.agents
                                        .filter((agent) => agent.status !== 'deleted')
                                        .map((agent) => agent.id),
                                    )}
                                />
                                {paused ? (
                                  <span
                                    className="tree-row__paused"
                                    role="img"
                                    aria-label="Sandbox paused"
                                    title="Sandbox paused"
                                  >
                                    <IconPauseOutline16 />
                                  </span>
                                ) : undefined}
                              </>
                            }
                            actions={sandboxActions(
                              sandbox,
                              permissions,
                              refreshError !== undefined,
                              lifecycleBusy,
                              busySandboxIds?.has(sandbox.id) === true,
                            )}
                            onNavigate={onNavigate}
                            onToggle={childrenAvailable ? () => toggle(sandboxNode) : undefined}
                            onAction={(kind) =>
                              kind === 'new_agent'
                                ? onAction({ kind, sandboxId: sandbox.id })
                                : onAction({
                                    kind: kind as
                                      | 'new_workspace'
                                      | 'pause_sandbox'
                                      | 'revive_sandbox'
                                      | 'delete_sandbox',
                                    sandboxId: sandbox.id,
                                  })
                            }
                          />
                          {sandboxExpanded ? (
                            <div role="group">
                              {workspaces
                                .filter((workspace) => workspace.status !== 'removed')
                                .map((workspace) => {
                                  const workspaceNode = `workspace:${workspace.id}`;
                                  const workspaceReady = workspace.status === 'ready';
                                  const workspaceFailed = workspace.status === 'failed';
                                  const selectedAgent =
                                    route.kind === 'agent'
                                      ? workspace.agents.find((agent) => agent.id === route.agentId)
                                      : undefined;
                                  const workspaceExpanded =
                                    workspaceReady && !collapsed.has(workspaceNode);
                                  return (
                                    <div className="resource-tree__branch" key={workspace.id}>
                                      <TreeRow
                                        nodeId={workspaceNode}
                                        parentId={sandboxNode}
                                        level={2}
                                        route={
                                          workspaceFailed
                                            ? { kind: 'workspace', workspaceId: workspace.id }
                                            : undefined
                                        }
                                        selected={
                                          workspaceFailed &&
                                          route.kind === 'workspace' &&
                                          route.workspaceId === workspace.id
                                        }
                                        label={
                                          selectedAgent?.runtimeMetadata?.workingDirectory?.trim() ||
                                          workspace.path ||
                                          workspace.name
                                        }
                                        icon={
                                          workspace.source.type === 'git_repository' ? (
                                            <IconBranchOutline16 />
                                          ) : (
                                            <IconFolderOpenOutline16 />
                                          )
                                        }
                                        meta={
                                          <>
                                            {parked &&
                                            !['failed', 'removed', 'removing'].includes(
                                              workspace.status,
                                            )
                                              ? presentation.label
                                              : statusLabel(
                                                  sandboxChildStatus(sandbox, workspace.status),
                                                )}
                                            <TreeFactsSummary
                                              channelFacts={channelFacts}
                                              facts={facts}
                                              ids={(
                                                originalWorkspaces.find(
                                                  (value) => value.id === workspace.id,
                                                )?.agents ?? []
                                              )
                                                .filter((agent) => agent.status !== 'deleted')
                                                .map((agent) => agent.id)}
                                            />
                                          </>
                                        }
                                        expanded={workspaceReady ? workspaceExpanded : undefined}
                                        actions={workspaceActions(
                                          workspace,
                                          permissions,
                                          sandboxBusy,
                                          runtimeAvailable,
                                        )}
                                        onNavigate={onNavigate}
                                        onToggle={
                                          workspaceReady ? () => toggle(workspaceNode) : undefined
                                        }
                                        onAction={(kind) =>
                                          kind === 'new_agent'
                                            ? onAction({ kind, workspaceId: workspace.id })
                                            : onAction({
                                                kind: 'remove_workspace',
                                                workspaceId: workspace.id,
                                              })
                                        }
                                      />
                                      {workspaceExpanded ? (
                                        <div role="group">
                                          {workspace.agents
                                            .filter((agent) => agent.status !== 'deleted')
                                            .map((agent) => {
                                              const effectiveStatus = sandboxChildStatus(
                                                sandbox,
                                                agent.status,
                                              );
                                              const task = parked
                                                ? taskPresentation(
                                                    agent,
                                                    sandbox,
                                                    refreshError ? 'stale' : 'fresh',
                                                    currentTask,
                                                  )
                                                : undefined;
                                              const observation =
                                                task === undefined
                                                  ? undefined
                                                  : {
                                                      label: task.label,
                                                      tone:
                                                        task.group === 'Needs attention'
                                                          ? ('error' as const)
                                                          : statusTone(agent.status),
                                                    };
                                              const status = refreshError
                                                ? undefined
                                                : agent.status;
                                              return (
                                                <TreeRow
                                                  key={agent.id}
                                                  nodeId={`agent:${agent.id}`}
                                                  parentId={workspaceNode}
                                                  level={3}
                                                  label={agentDisplayTitle(agent)}
                                                  icon={agentSignal(
                                                    agentListAction(status, sandbox, task),
                                                    observation?.label ??
                                                      statusLabel(effectiveStatus),
                                                    16,
                                                    sandbox.color,
                                                  )}
                                                  route={{ kind: 'agent', agentId: agent.id }}
                                                  selected={
                                                    route.kind === 'agent' &&
                                                    route.agentId === agent.id
                                                  }
                                                  dimmed={paused}
                                                  meta={stateMeta(
                                                    effectiveStatus,
                                                    <>
                                                      <CreationBadge id={agent.id} facts={facts} />
                                                      <ChannelBadge
                                                        id={agent.id}
                                                        facts={channelFacts}
                                                      />
                                                      {Number.isFinite(taskActivityOrder(agent)) ? (
                                                        <time dateTime={agent.taskActivityAt}>
                                                          {activityLabel(agent.taskActivityAt!)}
                                                        </time>
                                                      ) : (
                                                        <span>Task time unavailable</span>
                                                      )}
                                                    </>,
                                                    observation,
                                                  )}
                                                  actions={agentActions(agent.status, permissions, {
                                                    sandbox:
                                                      agent.sandboxId === sandbox.id
                                                        ? sandbox
                                                        : undefined,
                                                    viewerId,
                                                    busy: sandboxBusy,
                                                    stale: !!refreshError,
                                                  })}
                                                  onNavigate={onNavigate}
                                                  onAction={(kind) =>
                                                    selectAgentAction(
                                                      kind,
                                                      agent.id,
                                                      onAction,
                                                      agent.sandboxId === sandbox.id
                                                        ? sandbox.id
                                                        : undefined,
                                                    )
                                                  }
                                                />
                                              );
                                            })}
                                        </div>
                                      ) : null}
                                    </div>
                                  );
                                })}
                            </div>
                          ) : null}
                        </div>
                      );
                    })}
                    {group.items.length === 0 ? (
                      <p className="resource-tree__empty">
                        {statusFilter === 'all'
                          ? `No ${group.label.toLowerCase()} sandboxes`
                          : sandboxFilterEmpty(statusFilter)}
                      </p>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </section>
          );
        })
      )}
      {view === 'sandbox' && statusFilter === 'all' && visibleItems.length === 0 ? (
        <p className="resource-tree__empty">No Sandboxes</p>
      ) : null}
    </div>
  );
}
