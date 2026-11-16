import {settingsRead, settingsConflict, retentionCase} from './settings-browser.mjs';
import {settingsPatch} from './settings-api.mjs';
import {settingsAudit, activityFull} from './activity.mjs';
import {idlePreview} from './idle-preview.mjs';
import {admissionRead} from './admission-read.mjs';
import {activityTable} from './activity-browser.mjs';
import {spendRead, spendObservation} from './spend-api.mjs';
import {admissionStop, admissionResume} from './admission-write.mjs';
import {perUserLimit, visitorLimits, measuredVisibility, idlePause} from './enforcement.mjs';
import {automaticPause, defaultsMarketplace} from './settings-lifecycle.mjs';
import {triggerSetting, triggerPause, triggerResume} from './triggers.mjs';
import {spendAvailability, spendPeriod, defaultOwnerPolicy} from './spend-browser.mjs';
import {ownerOverrides, requestModels} from './spend-lists.mjs';
import {crossOwnerReads, crossOwnerInteraction, recovery, noCustodyEscalation} from './cross-owner.mjs';
import {crossOwnerControls} from './cross-controls.mjs';
import {sandboxCommands, orgLifecycle} from './sandbox-controls.mjs';
import {orgInventory, orgInteraction, orgHistory} from './organization.mjs';
import {orgSharing, linkServiceBoundary} from './organization-sharing.mjs';
import {readFileSync} from 'node:fs';

const implementations = [settingsRead, settingsConflict, retentionCase, idlePreview,
  settingsPatch, settingsAudit, activityTable, activityFull, admissionRead, admissionStop, admissionResume,
  spendRead, spendObservation, perUserLimit, visitorLimits, measuredVisibility, idlePause,
  automaticPause, defaultsMarketplace, triggerSetting, triggerPause, triggerResume,
  spendAvailability, spendPeriod, defaultOwnerPolicy, ownerOverrides, requestModels,
  crossOwnerReads, crossOwnerInteraction, crossOwnerControls, sandboxCommands, recovery, noCustodyEscalation,
  orgInventory, orgInteraction, orgSharing, linkServiceBoundary, orgLifecycle, orgHistory];

const setups = JSON.parse(readFileSync(new URL('./setup-requirements.json', import.meta.url))).factories;
export const cases = implementations.map(definition => {
  const setup = setups.find(entry => entry.case_ids.includes(definition.id));
  if (!setup) throw Error('admin_case_setup_mapping_missing');
  // A generic adapter object cannot make an absent recipe count as executable.
  // Parent installs these capabilities only for concrete authentic factories.
  const capability = 'admin-setup-' + setup.name.replaceAll('_', '-');
  return {...definition, fixtures: [...definition.fixtures, capability], async run(ctx) {
    if (typeof ctx.fixture(capability) !== 'function') throw Error('admin_setup_capability_not_factory');
    return definition.run(ctx);
  }};
});
