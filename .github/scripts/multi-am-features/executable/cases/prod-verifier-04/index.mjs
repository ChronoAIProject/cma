import {profileCases} from './profiles.mjs';
import {usageCases} from './usage.mjs';
import {recoveryCases} from './recovery.mjs';
import {metadataCases} from './metadata.mjs';
import {documentCases} from './documents.mjs';

export const cases = [...profileCases, ...usageCases, ...recoveryCases, ...metadataCases, ...documentCases];
