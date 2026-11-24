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

function modelNames(models: readonly AgentModelOption[]): Map<string, string> {
  const names = models.map((model) => ({
    model,
    key: model.label.replace(/[()]/g, ' ').trim().replace(/\s+/g, ' ').toLowerCase(),
  }));
  const counts = new Map<string, number>();
  for (const { key } of names) counts.set(key, (counts.get(key) ?? 0) + 1);

  // The catalog ID distinguishes rows whose labels differ only by presentation.
  return new Map(
    names.map(({ model, key }) => [
      model.id,
      counts.get(key) === 1 ? model.label : `${model.label} · ${model.id}`,
    ]),
  );
}

/** The catalog's name for a model, disambiguated with its ID when needed. */
export function modelLabel(models: readonly AgentModelOption[], model: string): string {
  return modelNames(models).get(model) ?? model;
}

/**
 * The model list's rows: the catalog, plus the current model when the
 * catalog no longer lists it, each unavailable row keeping its reason (§12.3).
 */
export function modelEntries(models: readonly AgentModelOption[], current: string): MenuEntry[] {
  const names = modelNames(models);
  return [
    ...(names.has(current)
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
      label: names.get(model.id) ?? model.label,
      disabled: !model.available,
      reason: model.available ? undefined : 'This Agent cannot select this model yet.',
    })),
  ];
}
