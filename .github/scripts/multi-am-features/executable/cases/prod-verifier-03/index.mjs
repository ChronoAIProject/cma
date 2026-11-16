import {browserCases} from './browser.mjs';
import {platformCases} from './platform.mjs';
import {toolCases} from './tools.mjs';
import {moreBrowserCases} from './browser-more.mjs';
import {channelUiCases} from './channel-ui.mjs';

import {githubCases} from './github.mjs';

import {mcpCases} from './mcp.mjs';

import {mediaToolCases} from './media-tools.mjs';

export const cases = [...browserCases, ...moreBrowserCases, ...channelUiCases, ...platformCases, ...toolCases, ...githubCases, ...mcpCases, ...mediaToolCases]
  .sort((a, b) => a.id.localeCompare(b.id));
