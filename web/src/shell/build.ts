/**
 * Which build of the frontend is on screen — docs/Web_Style.md §6.
 *
 * The status band names it because a person reporting something odd, and the
 * person reading that report, need to be talking about the same bundle. It is
 * baked in at build time rather than fetched: a build identifier the running
 * page asks a server for is one that can disagree with the code answering the
 * question.
 *
 * `dev` when nothing set it, which is the honest answer for a local run and is
 * what CI's image build produces. The publisher passes the release version.
 */
declare const __CHRONO_BUILD__: string | undefined;

export const BUILD: string = typeof __CHRONO_BUILD__ === 'string' ? __CHRONO_BUILD__ : 'dev';

/**
 * The public API base for the deployment serving this bundle.
 *
 * The web image proxies every public interface on its own origin, so the
 * browser's origin is the only value that cannot drift from the deployment a
 * person actually opened. Kept beside the baked build id as shell metadata;
 * no component invents or hardcodes an endpoint of its own.
 */
export const API_BASE_URL: string = window.location.origin;
