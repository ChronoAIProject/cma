import { useState } from 'react';
import { Input } from '../../ui/Input';
import type { CapacityBounds, SandboxProfileContent } from '../client';
import { ChoiceGroup } from '../ChoiceGroup';
import { Field } from '../Field';
import { fieldProps, type EditorState } from '../ProfileEditor';

const PRESETS = {
  standard: { cpuMillis: 2000, memoryMib: 4096 },
  large: { cpuMillis: 4000, memoryMib: 8192 },
};

export function ResourceFields({
  editor,
  bounds,
}: {
  readonly editor: EditorState<SandboxProfileContent>;
  readonly bounds?: CapacityBounds;
}) {
  const [custom, setCustom] = useState(false);
  const content = editor.draft.content;
  const preset = Object.entries(PRESETS).find(
    ([, value]) => value.cpuMillis === content.cpuMillis && value.memoryMib === content.memoryMib,
  )?.[0] as 'standard' | 'large' | undefined;
  const mode = custom ? 'custom' : (preset ?? 'custom');
  const allowed = (value: (typeof PRESETS)['standard']) =>
    bounds !== undefined &&
    value.cpuMillis >= bounds.minCpuMillis &&
    value.cpuMillis <= bounds.maxCpuMillis &&
    value.memoryMib >= bounds.minMemoryMib &&
    value.memoryMib <= bounds.maxMemoryMib;
  const change = (next: Partial<SandboxProfileContent>) =>
    editor.change({ ...editor.draft, content: { ...content, ...next } });
  return (
    <>
      <div className="profile-field">
        <ChoiceGroup
          label="Size"
          value={mode}
          options={[
            { value: 'standard', label: 'Standard', disabled: !allowed(PRESETS.standard) },
            { value: 'large', label: 'Large', disabled: !allowed(PRESETS.large) },
            { value: 'custom', label: 'Custom' },
          ]}
          disabled={editor.locked || bounds === undefined}
          describedBy="profile-size-help"
          onChange={(next) => {
            setCustom(next === 'custom');
            if (next !== 'custom') change(PRESETS[next]);
          }}
        />
        <p id="profile-size-help" className="profile-field__help">
          Standard uses 2 cores and 4 GiB; Large uses 4 cores and 8 GiB; Custom stays within
          deployment limits.
        </p>
      </div>
      <div className="profile-field-pair">
        <Field
          id="profile-cpuMillis"
          label="CPU cores"
          help={
            bounds === undefined
              ? 'The deployment must provide its CPU range before this profile can be saved.'
              : `Choose ${String(bounds.minCpuMillis / 1000)} to ${String(bounds.maxCpuMillis / 1000)} CPU cores.`
          }
          error={editor.errors.cpuMillis}
        >
          <Input
            {...fieldProps(editor, 'cpuMillis')}
            type="number"
            step="0.001"
            min={bounds === undefined ? undefined : bounds.minCpuMillis / 1000}
            max={bounds === undefined ? undefined : bounds.maxCpuMillis / 1000}
            value={Number.isFinite(content.cpuMillis) ? content.cpuMillis / 1000 : ''}
            disabled={editor.locked || bounds === undefined || mode !== 'custom'}
            onChange={(event) => change({ cpuMillis: event.target.valueAsNumber * 1000 })}
          />
        </Field>
        <Field
          id="profile-memoryMib"
          label="Memory (GiB)"
          help={
            bounds === undefined
              ? 'The deployment must provide its memory range before this profile can be saved.'
              : `Choose ${String(bounds.minMemoryMib / 1024)} to ${String(bounds.maxMemoryMib / 1024)} GiB of memory.`
          }
          error={editor.errors.memoryMib}
        >
          <Input
            {...fieldProps(editor, 'memoryMib')}
            type="number"
            step="any"
            min={bounds === undefined ? undefined : bounds.minMemoryMib / 1024}
            max={bounds === undefined ? undefined : bounds.maxMemoryMib / 1024}
            value={Number.isFinite(content.memoryMib) ? content.memoryMib / 1024 : ''}
            disabled={editor.locked || bounds === undefined || mode !== 'custom'}
            onChange={(event) => change({ memoryMib: event.target.valueAsNumber * 1024 })}
          />
        </Field>
      </div>
      {bounds === undefined ? <p role="status">Resource limits are unavailable.</p> : null}
    </>
  );
}
