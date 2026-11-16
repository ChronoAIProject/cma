import {profileCases} from './profiles.mjs';
import {usageCases} from './usage.mjs';
import {recoveryCases} from './recovery.mjs';
import {metadataCases} from './metadata.mjs';
import {documentCases} from './documents.mjs';
import {capabilityCases} from './capabilities.mjs';
import {launchCases} from './launches.mjs';
import {hierarchyCases} from './hierarchy.mjs';
import {delegationCases} from './delegation.mjs';
import {compatibilityCases} from './compatibility.mjs';

export const cases = [...profileCases, ...usageCases, ...recoveryCases, ...metadataCases, ...documentCases, ...capabilityCases, ...launchCases, ...hierarchyCases, ...delegationCases, ...compatibilityCases];
