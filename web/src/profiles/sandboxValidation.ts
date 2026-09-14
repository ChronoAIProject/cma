import type { CapacityBounds, ProfileDraft, SandboxProfileContent } from './client';
import { validateMetadata, type FieldErrors } from './validation';
import { commandErrors } from './workspaceValidation';

const RESERVED_NAMES = new Set(['PATH', 'HOME', 'NYXID_URL', 'NYXID_ACCESS_TOKEN', 'CODEX_HOME']);

export function validateSandboxProfile(
  draft: ProfileDraft<SandboxProfileContent>,
  bounds?: CapacityBounds,
): FieldErrors {
  const value = draft.content;
  const errors: Record<string, string> = {
    ...validateMetadata(draft),
    ...commandErrors(value.softwareInstallCommands, 32, 'softwareInstallCommands'),
  };
  if (
    draft.availability !== undefined &&
    (!Number.isInteger(draft.availability.warmupCount) ||
      draft.availability.warmupCount < 1 ||
      draft.availability.warmupCount > 5)
  )
    errors.warmupCount = 'Choose a whole number from 1 to 5.';
  if (bounds === undefined)
    errors.cpuMillis = 'Resource limits could not be loaded. Try again before saving.';
  else {
    if (
      !Number.isInteger(value.cpuMillis) ||
      value.cpuMillis < bounds.minCpuMillis ||
      value.cpuMillis > bounds.maxCpuMillis
    )
      errors.cpuMillis = `Choose between ${String(bounds.minCpuMillis / 1000)} and ${String(bounds.maxCpuMillis / 1000)} cores.`;
    if (
      !Number.isInteger(value.memoryMib) ||
      value.memoryMib < bounds.minMemoryMib ||
      value.memoryMib > bounds.maxMemoryMib
    )
      errors.memoryMib = `Choose between ${String(bounds.minMemoryMib / 1024)} and ${String(bounds.maxMemoryMib / 1024)} GiB.`;
  }
  if (!Number.isInteger(value.maxWorkspaces) || value.maxWorkspaces < 1 || value.maxWorkspaces > 16)
    errors.maxWorkspaces = 'Choose between 1 and 16 workspaces.';
  if (
    !Number.isInteger(value.maxAgentsPerWorkspace) ||
    value.maxAgentsPerWorkspace < 1 ||
    value.maxAgentsPerWorkspace > 16
  )
    errors.maxAgentsPerWorkspace = 'Choose between 1 and 16 agents per workspace.';
  if (
    value.workspaceProfiles.length > value.maxWorkspaces ||
    value.workspaceProfiles.some((entry) => !Number.isInteger(entry.revision) || entry.revision < 1)
  )
    errors.workspaceProfiles = 'Choose published workspace profiles within the workspace limit.';
  if (value.environmentVariables.length > 64)
    errors.environmentVariables = 'Use at most 64 environment variables.';
  const names = new Set<string>();
  value.environmentVariables.forEach((entry, index) => {
    const field = `environmentVariables.${String(index)}`;
    if (
      !/^[A-Z_][A-Z0-9_]*$/.test(entry.name) ||
      RESERVED_NAMES.has(entry.name) ||
      entry.name.startsWith('CHRONO_')
    )
      errors[`${field}.name`] = 'Use an uppercase name that is not reserved by the system.';
    else if (names.has(entry.name))
      errors[`${field}.name`] = 'Each environment variable needs a different name.';
    names.add(entry.name);
    if (entry.value.includes('\0'))
      errors[`${field}.value`] = 'Remove null characters from this value.';
  });
  return errors;
}
