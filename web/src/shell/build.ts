/**
 * Which build of the frontend is on screen — docs/Web_Style.md §6.
 *
 * The account menu names it because a person reporting something odd, and the
 * person reading that report, need to be talking about the same bundle. It is
 * baked in at build time rather than fetched: a build identifier the running
 * page asks a server for is one that can disagree with the code answering the
 * question.
 *
 * Local runs are explicitly local. Deployable artifacts require both a
 * release/git version and a full revision before Vite builds the bundle.
 */
declare const __CHRONO_BUILD__: string | undefined;
declare const __CHRONO_BUILD_SHA__: string | undefined;
declare const __CHRONO_BUILD_LOCAL__: boolean | undefined;

export const BUILD: string = typeof __CHRONO_BUILD__ === 'string' ? __CHRONO_BUILD__ : 'dev';
const revision = typeof __CHRONO_BUILD_SHA__ === 'string' ? __CHRONO_BUILD_SHA__ : '';
const local = typeof __CHRONO_BUILD_LOCAL__ === 'boolean' ? __CHRONO_BUILD_LOCAL__ : true;

export const BUILD_INFO = Object.freeze({
  version: BUILD,
  revision,
  local,
  label: local ? `${BUILD} (local)` : BUILD,
  copyText: `CMA ${BUILD} (${revision || 'local'})${local && revision ? ' (local)' : ''}`,
});

/**
 * The public API base for the deployment serving this bundle.
 *
 * The deployment keeps every public interface on the browser's origin. Its
 * Gateway routes API, auth, and discovery paths directly to the control plane,
 * while nginx provides the same contract when the image is reached without
 * that split. The origin is therefore the only value that cannot drift from
 * the deployment a person actually opened. Kept beside the baked build id as shell metadata;
 * no component invents or hardcodes an endpoint of its own.
 */
export const API_BASE_URL: string = window.location.origin;
