import type { AgentModelOption, AgentSettingsView } from '../api/client';
import type { MenuEntry } from '../ui/Menu';

type Effort = AgentSettingsView['reasoning_effort'];
type Permission = AgentSettingsView['permission_mode'];

/**
 * Every effort level the settings contract accepts, in order.
 *
 * The model catalog reports no narrower set per model, so these are the
 * slider's stops for every model.
 */
export const EFFORT_STOPS: readonly { readonly value: Effort; readonly label: string }[] = [
  { value: 'minimal', label: 'Minimal' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'x_high', label: 'Very high' },
];

export const PERMISSION_CHOICES: readonly { readonly value: Permission; readonly label: string }[] =
  [
    { value: 'guarded', label: 'Guarded' },
    { value: 'autonomous', label: 'Autonomous' },
  ];

export function effortLabel(effort: Effort): string {
  return EFFORT_STOPS.find((stop) => stop.value === effort)?.label ?? effort;
}

export function permissionLabel(permission: Permission): string {
  return PERMISSION_CHOICES.find((choice) => choice.value === permission)?.label ?? permission;
}

/** The catalog's name for a model, or its id when the catalog does not list it. */
export function modelLabel(models: readonly AgentModelOption[], model: string): string {
  return models.find((option) => option.id === model)?.label ?? model;
}

/**
 * The model list's rows: the catalog, plus the current model when the
 * catalog no longer lists it, each unavailable row keeping its reason (§12.3).
 */
export function modelEntries(models: readonly AgentModelOption[], current: string): MenuEntry[] {
  return [
    ...(models.some((model) => model.id === current)
      ? []
      : [
          {
            id: current,
            label: current,
            disabled: true,
            reason: 'Current model is not in the available catalog.',
          },
        ]),
    ...models.map((model) => ({
      id: model.id,
      label: model.label,
      disabled: !model.available,
      reason: model.available ? undefined : 'This Agent cannot select this model yet.',
    })),
  ];
}
