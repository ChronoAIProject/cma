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
