import { useState } from 'react';
import type { AgentModelOption, AgentSettingsView } from '../api/client';
import { Button } from '../ui/Button';
import { Menu } from '../ui/Menu';
import { Popover } from '../ui/Popover';
import {
  IconChevronDownOutline14,
  IconChevronRightOutline14,
  IconRefreshOutline14,
} from '../ui/icons';
import { EffortSlider } from './EffortSlider';
import { EFFORT_STOPS, effortLabel, modelEntries, modelLabel } from './settingsChoices';

/** What the catalog says about choosing a model, under the model row. */
function CatalogNotice({
  models,
  loading,
  error,
  onRetry,
}: {
  readonly models: readonly AgentModelOption[];
  readonly loading: boolean;
  readonly error?: string;
  readonly onRetry: () => void;
}) {
  if (loading) return <p role="status">Loading available models…</p>;
  if (error !== undefined)
    return (
      <div role="alert" className="model-effort__catalog-error">
        <p>{error}</p>
        <Button size="sm" onClick={onRetry}>
          Retry models
        </Button>
      </div>
    );
  if (models.length === 0) return <p role="status">No models are available for this Agent.</p>;
  if (models.every((model) => !model.available))
    return (
      <p role="status">
        Model changes are unavailable for this Agent. You can still change reasoning effort and
        permissions.
      </p>
    );
  return null;
}

/**
 * Model and reasoning effort as one chip and its card — the shared style contracts
 *
 * The two are chosen together in practice — a model and how hard it thinks —
 * so they share one control instead of two pickers. The card changes only
 * the draft: applying it stays the explicit, separate step its owner keeps.
 */
export function ModelEffortPicker({
  value,
  saved,
  models,
  modelsLoading,
  modelsError,
  onRetryModels,
  disabled,
  onChange,
}: {
  readonly value: AgentSettingsView;
  /** The Agent's confirmed settings, which Reset returns to. */
  readonly saved?: AgentSettingsView;
  readonly models: readonly AgentModelOption[];
  readonly modelsLoading: boolean;
  readonly modelsError?: string;
  readonly onRetryModels: () => void;
  /** An update is pending or retained, so the controls hold. */
  readonly disabled: boolean;
  readonly onChange: (value: AgentSettingsView) => void;
}) {
  const [open, setOpen] = useState(false);
  const [listOpen, setListOpen] = useState(false);
  const model = modelLabel(models, value.model);
  const effort = effortLabel(value.reasoning_effort);
  const matchesSaved =
    saved === undefined ||
    (saved.model === value.model && saved.reasoning_effort === value.reasoning_effort);
  return (
    <Popover
      open={open}
      label="Model and reasoning effort"
      placement="agent-settings__model"
      cardClassName="model-effort"
      onClose={() => {
        setListOpen(false);
        setOpen(false);
      }}
      anchor={
        <button
          type="button"
          className="composer-chip"
          aria-label={`Model and reasoning effort: ${model}, ${effort}`}
          aria-haspopup="dialog"
          aria-expanded={open}
          title={`Model: ${model} · Reasoning effort: ${effort}`}
          onClick={() => setOpen((was) => !was)}
        >
          <span className="composer-chip__label">
            {model} <span className="model-effort__chip-effort">{effort}</span>
          </span>
          <IconChevronDownOutline14 className="composer-chip__caret" />
        </button>
      }
    >
      <div className="model-effort__head">
        <p className="model-effort__effort">{effort}</p>
        <Button
          size="sm"
          icon={<IconRefreshOutline14 />}
          aria-label="Reset to current settings"
          title="Reset to current settings"
          disabled={disabled || matchesSaved}
          onClick={() => {
            if (saved !== undefined)
              onChange({ ...value, model: saved.model, reasoning_effort: saved.reasoning_effort });
          }}
        />
      </div>
      <Menu
        open={listOpen && !disabled}
        placement="model-effort__list"
        side="top"
        align="start"
        dense
        label="Model"
        selectedId={value.model}
        items={modelEntries(models, value.model)}
        onClose={() => setListOpen(false)}
        onSelect={(id) => {
          setListOpen(false);
          if (id !== value.model) onChange({ ...value, model: id });
        }}
        anchor={
          <button
            type="button"
            className="model-effort__model"
            aria-label="Model"
            aria-haspopup="menu"
            aria-expanded={listOpen && !disabled}
            title={`Model: ${model}`}
            disabled={disabled || modelsLoading || models.length === 0}
            onClick={() => setListOpen((was) => !was)}
          >
            <span className="model-effort__model-name">{model}</span>
            <IconChevronRightOutline14 />
          </button>
        }
      />
      <div className="model-effort__catalog">
        <CatalogNotice
          models={models}
          loading={modelsLoading}
          error={modelsError}
          onRetry={onRetryModels}
        />
      </div>
      <EffortSlider
        label="Reasoning effort"
        value={value.reasoning_effort}
        stops={EFFORT_STOPS}
        disabled={disabled}
        onChange={(reasoning_effort) => onChange({ ...value, reasoning_effort })}
      />
    </Popover>
  );
}
