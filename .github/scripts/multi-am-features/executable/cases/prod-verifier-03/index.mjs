import {browserCases} from './browser.mjs';
import {platformCases} from './platform.mjs';

export const cases = [...browserCases, ...platformCases].sort((a, b) => a.id.localeCompare(b.id));
