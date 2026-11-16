import {browserCases} from './browser.mjs';
import {platformCases} from './platform.mjs';
import {toolCases} from './tools.mjs';
import {moreBrowserCases} from './browser-more.mjs';
import {channelUiCases} from './channel-ui.mjs';

import {githubCases} from './github.mjs';

import {mcpCases} from './mcp.mjs';

import {mediaToolCases} from './media-tools.mjs';

import {nativePlatformCases} from './native-platform.mjs';

import {botCases} from './bots.mjs';

import {bindingCases} from './bindings.mjs';
import {setups as existingSetups} from './setup.mjs';
import {cases as remainingCases, setups as remainingSetups} from '../developer-17/eg-remaining/index.mjs';
export const setups = {...existingSetups, ...remainingSetups};

import {interactionCases} from './interactions.mjs';

import {usageDetailCases} from './usage-details.mjs';

export const cases = [...browserCases, ...moreBrowserCases, ...channelUiCases, ...platformCases, ...toolCases, ...githubCases, ...mcpCases, ...mediaToolCases, ...nativePlatformCases, ...botCases, ...bindingCases, ...interactionCases, ...usageDetailCases, ...remainingCases]
  .sort((a, b) => a.id.localeCompare(b.id));
