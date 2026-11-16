import {settingsRead, settingsConflict, retentionCase} from './settings-browser.mjs';
import {settingsPatch} from './settings-api.mjs';
import {settingsAudit, activityFull} from './activity.mjs';
import {idlePreview} from './idle-preview.mjs';
import {admissionRead} from './admission-read.mjs';
import {activityTable} from './activity-browser.mjs';
import {spendRead, spendObservation} from './spend-api.mjs';
import {admissionStop, admissionResume} from './admission-write.mjs';

export const cases = [settingsRead, settingsConflict, retentionCase, idlePreview,
  settingsPatch, settingsAudit, activityTable, activityFull, admissionRead, admissionStop, admissionResume,
  spendRead, spendObservation];
