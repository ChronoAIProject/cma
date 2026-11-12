# CMA Research Profile

This is the local preparation record for a research Agent in its own Worktree
inside `/home/cma/trureturing`. It contains profile blueprints and a home-local
Linux x64 installation recipe, not published profile references or a claim that
the research journey has run successfully. No profile, Trigger, remote branch,
or production resource was created while preparing this document.

## Source And Scope

The public repository is
`https://github.com/the-omega-institute/trureturing`. At
**2026-09-06 22:45:10 UTC**, `git ls-remote --symref ... HEAD` reported the default
branch `refs/heads/dev` at:

```text
5b9d526360dfb147036db8b0f663c408bd8cf33d
```

The earlier preparation input was
`182913fa3185edce7c0ba51fa32ae6d79d6db614`. Eighteen relevant instruction,
toolchain, manifest, build, cache, and inspection files were compared at the two
commits and were byte-for-byte identical. This does not imply that the entire
repository is unchanged. Source links below use the refreshed observed commit.

The Workspace Profile deliberately omits `location.reference`: CMA clones the
public repository's default branch. Setup **asserts** that the cloned `HEAD`
equals the observed commit and fails closed otherwise. It never checks out a
different commit. CMA records the resolved source commit before setup and uses
that record for new Agent Worktrees; changing only the main checkout in setup
would not change that recorded base. The current `reference` contract is a
branch or tag, not a raw-SHA clone option.

If the default branch advances before creation, reinspect its new `HEAD`, update
the assertion and this record, and publish new profile revisions in dependency
order. Do not remove the assertion, substitute an invented tag, or silently
reuse installation assumptions after an upstream toolchain change.

### Inspected Contracts

All paths in this table are relative to the
[observed source tree](https://github.com/the-omega-institute/trureturing/tree/5b9d526360dfb147036db8b0f663c408bd8cf33d).

| Source | Preparation consequence |
|---|---|
| `AGENTS.md`, `CLAUDE.md`, `README.md`, `agents/CONTEXT.md` | The research Agent must read all governing instructions before research edits. `CLAUDE.md` is 241,130 bytes; do not mistake an excerpt for a complete read. |
| `lean-toolchain` | Exact toolchain `leanprover/lean4:v4.33.0`. |
| `lakefile.toml`, `lake-manifest.json` | mathlib `v4.33.0`, locked to `db584cd6d46c92f209a44c0f1c829460d327499d`; preserve the complete dependency lockfile. Default targets include `Trureturing` and `LeanInformationAudit`. |
| `global.json`, `Directory.Build.props`, `Directory.Packages.props`, `tools/StrataLint.Cli/packages.lock.json` | Install .NET SDK `10.0.103` exactly; target is `net10.0`. Restore with `--locked-mode`. Do not rely on `global.json`'s `latestMinor` roll-forward to select a different SDK. |
| `Makefile`, `tools/Makefile`, `tools/scripts/dotnet-build.sh` | Start with `make help`; `make -C tools dotnet` performs locked restore and a warnings-as-errors build. |
| `tools/scripts/worktree/lean-cache-ensure.sh`, `tools/scripts/worktree/lean-cache-run.sh` | Cache preparation itself calls .NET. Each actual Agent Worktree needs a private `.lake`; Linux uses the repository's cache machinery, not an APFS donor or symlinked cache. |
| `tools/lean-inspector/inspect.sh`, `tools/scripts/report/lean-report.sh` | Produce source-bound Lean evidence, its SHA-256 sidecar, and material archive; run .NET admission and selftests separately. |

The refreshed `CLAUDE.md` SHA-256 is
`219be688c9f6fdc2cd9670cdd3528dd2cbc0da0d57cece45ca7b3412373055c6`.
Its bytes were compared, but this CMA documentation task did not claim a full
semantic reading of that upstream file. The standing prompt requires that read
in the actual research Worktree.

## Profile Blueprints

These are REST **create-request blueprints**, with valid scalar fields and empty
assembly slots. They are not ready-to-publish payloads. Assemble them as follows:

1. Replace the Agent's empty `system_prompt` with the standing-prompt block below.
2. Replace the Workspace's `setup_commands` with the three numbered setup blocks,
   each block as one string, preserving order and newlines.
3. Replace the Sandbox's `software_install_commands` with the eight numbered
   install blocks, each block as one string, preserving order and newlines.
4. Populate `agent_profiles` and then `workspace_profiles` only with actual
   published `{ "id": ..., "revision": ... }` responses. No revision or ID is
   predicted in this document.

The proposed capacity is four CPUs and 16 GiB for one active research Agent.
This is a starting resource budget, not a measured minimum or a guarantee that
the whole repository builds within it. Before saving, compare it with
`GET /api/v1/profile-authoring/bounds` and the operator's storage capacity. Allow
at least 20 GiB of initially free home storage for the toolchains and private
caches, then measure peak usage in the real smoke test. Profile fields do not
reserve disk space. Do not advertise ARM support from the x64 recipe.

<!-- cma-research:profiles -->
```json
{
  "agent": {
    "name": "Trureturing Research Agent",
    "description": "Local research in an isolated Trureturing Worktree with source-bound proof evidence.",
    "content": {
      "model": "gpt-5.6-sol",
      "thinking_effort": "high",
      "permission_level": "autonomous",
      "skills": [],
      "system_prompt": ""
    }
  },
  "workspace": {
    "name": "Trureturing Research Workspace",
    "description": "Public default-branch clone with a fail-closed inspected-commit assertion.",
    "content": {
      "location": {
        "type": "git_repository",
        "url": "https://github.com/the-omega-institute/trureturing"
      },
      "name": "trureturing",
      "setup_commands": [],
      "agent_profiles": []
    }
  },
  "sandbox": {
    "name": "Trureturing Research Sandbox",
    "description": "Home-local Linux x64 Lean 4.33.0, mathlib 4.33.0, elan 4.2.4, and .NET SDK 10.0.103.",
    "content": {
      "cpu_millis": 4000,
      "memory_mib": 16384,
      "nyxid_services": [],
      "agent_provider": "codex",
      "model_access": "chronoai_managed",
      "max_workspaces": 1,
      "max_agents_per_workspace": 1,
      "workspace_profiles": [],
      "software_install_commands": [],
      "environment_variables": []
    }
  }
}
```

`skills` means additional, resolved Ornn skill references. None was verified for
this recipe, so the array is empty. The runtime already supplies the `nyxid` and
`ornn-agent-manual-cli` baseline skills. Repository-local instructions and skills
remain repository material, not invented Ornn IDs. Additional NyxID services
are also empty; the server unions its baseline scope. Do not add service IDs,
credentials, or a model account to make an unverified journey appear complete.

The profile environment is empty because names such as `PATH`, `HOME`,
`CODEX_HOME`, and `CHRONO_*` are reserved. Instead, installation creates one
explicit environment script. Every later setup or research shell command must
source it. Shell startup files and one command's exported variables are not a
cross-command persistence mechanism. `autonomous` runs Codex in YOLO mode inside
the isolated CMA Sandbox: no provider approval prompts and no inner command
sandbox. Its configuration uses `never` and `danger-full-access`, and the ACP
bridge uses `agent-full-access`. The Sandbox's pinned NyxID scope still bounds
service access; autonomous execution does not add remote publication authority.

## Sandbox Installation

Run these only through a fresh Sandbox's software-install phase, as `cma`
UID/GID 10001, before credential delivery. The runtime image is Debian
trixie-slim with a read-only system filesystem and writable `/home/cma`.
Its Dockerfile supplies bash, curl, git, gzip, jq, ps (procps), Python, tar,
unzip, and xz, but does not explicitly install make, zstd, or ICU. `dpkg-deb` is
supplied by Debian's essential `dpkg` package and is asserted before use.
Extraction does not run
Debian maintainer scripts or alter the system package database.

All writes below are confined to the version-specific home prefix. The recipe
does not run `apt install`, sudo, a global installer, or an unverified remote
installer script. It retains downloaded archives for audit; they are part of
home storage and checkpoint size. Do not recursively clean the home to recover
a failed install. Preserve the operation receipt and use CMA's supported
recovery path. A resumed command rechecks the same pinned bytes.

The configured runtime command budget is currently 600 seconds per command and
1,800 seconds total. Downloads have shorter explicit timeouts; actual network,
extraction, restore, and storage timing remain smoke-test requirements.

### Install Command 1: Home And Environment

<!-- cma-research:install-01 -->
```bash
set -euo pipefail
test "$(uname -s)" = Linux
test "$(uname -m)" = x86_64
test "$(id -u)" = 10001
test "$(id -g)" = 10001
test "$HOME" = /home/cma
. /etc/os-release
test "$ID" = debian
test "$VERSION_CODENAME" = trixie
for research_tool in curl git tar dpkg-deb sha256sum sha512sum node; do
  command -v "$research_tool" >/dev/null
done
research_prefix=/home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103
test ! -L "$research_prefix"
umask 022
mkdir -p "$research_prefix/downloads" "$research_prefix/os" "$research_prefix/dotnet"
printf '%s\n' \
  'export RESEARCH_PREFIX=/home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103' \
  'export ELAN_HOME="$RESEARCH_PREFIX/elan"' \
  'export DOTNET_ROOT="$RESEARCH_PREFIX/dotnet"' \
  'export DOTNET_CLI_HOME="$RESEARCH_PREFIX/dotnet-home"' \
  'export DOTNET_CLI_TELEMETRY_OPTOUT=1' \
  'export DOTNET_NOLOGO=1' \
  'export DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=0' \
  'export PATH="$RESEARCH_PREFIX/os/usr/bin:$ELAN_HOME/bin:$DOTNET_ROOT:$PATH"' \
  'export LD_LIBRARY_PATH="$RESEARCH_PREFIX/os/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"' \
  'export LAKE_BIN="$ELAN_HOME/bin/lake"' \
  > "$research_prefix/env.sh"
test -w "$research_prefix"
df -Pk /home/cma
```

### Install Command 2: Pinned Debian Utilities

<!-- cma-research:install-02 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
while read -r research_sha research_path; do
  research_archive="$RESEARCH_PREFIX/downloads/${research_path##*/}"
  curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
    --connect-timeout 30 --max-time 120 --output "$research_archive" \
    "https://deb.debian.org/debian/$research_path"
  printf '%s  %s\n' "$research_sha" "$research_archive" | sha256sum --check -
  dpkg-deb --extract "$research_archive" "$RESEARCH_PREFIX/os"
done <<'DEBS'
70a9709e665383b06068ccd423d62ea191b65355f318a25565dbf324b254cec6 pool/main/m/make-dfsg/make_4.4.1-2_amd64.deb
c1bf762996de9ecba9b9d871e4928a8090f023a3e9e7fa3240b3d90f892a01dc pool/main/i/icu/libicu76_76.1-4_amd64.deb
6da09b6aad171ef9b50d6a2bb4c62f756b3561aac5e05b452b80516223e5e0f5 pool/main/libz/libzstd/zstd_1.5.7+dfsg-1_amd64.deb
DEBS
make --version
zstd --version
```

### Install Command 3: elan

<!-- cma-research:install-03 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
research_archive="$RESEARCH_PREFIX/downloads/elan-x86_64-unknown-linux-gnu.tar.gz"
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  --connect-timeout 30 --max-time 120 --output "$research_archive" \
  https://github.com/leanprover/elan/releases/download/v4.2.4/elan-x86_64-unknown-linux-gnu.tar.gz
printf '%s  %s\n' \
  42b94d4244e8353142c456ec0e4ca6528fd898a6c604d4059f494e706e431f63 \
  "$research_archive" | sha256sum --check -
tar -xzf "$research_archive" -C "$RESEARCH_PREFIX/downloads" elan-init
"$RESEARCH_PREFIX/downloads/elan-init" -y --default-toolchain none --no-modify-path
case "$(elan --version)" in
  'elan 4.2.4'*) ;;
  *) exit 1 ;;
esac
```

### Install Command 4: Lean Download

<!-- cma-research:install-04 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  --connect-timeout 30 --max-time 450 \
  --output "$RESEARCH_PREFIX/downloads/lean-4.33.0-linux.tar.zst" \
  https://github.com/leanprover/lean4/releases/download/v4.33.0/lean-4.33.0-linux.tar.zst
```

### Install Command 5: Lean Verification And Link

<!-- cma-research:install-05 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
research_archive="$RESEARCH_PREFIX/downloads/lean-4.33.0-linux.tar.zst"
printf '%s  %s\n' \
  4b3fb03c29a1e0a253fb1d11f9bae3725f19a0dc6fc09b3ea16d2c9df3349e2c \
  "$research_archive" | sha256sum --check -
tar --zstd -xf "$research_archive" -C "$RESEARCH_PREFIX"
elan toolchain link leanprover/lean4:v4.33.0 "$RESEARCH_PREFIX/lean-4.33.0-linux"
elan default leanprover/lean4:v4.33.0
lean --version
lake --version
```

### Install Command 6: .NET Download

<!-- cma-research:install-06 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  --connect-timeout 30 --max-time 450 \
  --output "$RESEARCH_PREFIX/downloads/dotnet-sdk-10.0.103-linux-x64.tar.gz" \
  https://builds.dotnet.microsoft.com/dotnet/Sdk/10.0.103/dotnet-sdk-10.0.103-linux-x64.tar.gz
```

### Install Command 7: .NET Verification And Extraction

<!-- cma-research:install-07 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
research_archive="$RESEARCH_PREFIX/downloads/dotnet-sdk-10.0.103-linux-x64.tar.gz"
printf '%s  %s\n' \
  bab94f13c57b2ac821d4924fe66084be9b44c41761ff7ff64522c8f7aba345659d31258401dcec31cc3cf6ccae1d012623075aca1c9b9165bcfe5ba9abda1c0c \
  "$research_archive" | sha512sum --check -
tar -xzf "$research_archive" -C "$DOTNET_ROOT"
test "$(dotnet --version)" = 10.0.103
dotnet --info
```

### Install Command 8: Final Toolchain Checks

<!-- cma-research:install-08 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
test -s "$RESEARCH_PREFIX/os/usr/lib/x86_64-linux-gnu/libicuuc.so.76.1"
test -s "$RESEARCH_PREFIX/os/usr/lib/x86_64-linux-gnu/libicui18n.so.76.1"
test "$(elan which lean)" = "$ELAN_HOME/toolchains/leanprover--lean4---v4.33.0/bin/lean"
case "$(lean --version)" in
  'Lean (version 4.33.0,'*) ;;
  *) exit 1 ;;
esac
test "$(dotnet --version)" = 10.0.103
make --version
zstd --version
lake --version
printf '%s\n' 'Lean 4.33.0; elan 4.2.4; .NET SDK 10.0.103; Linux x64' \
  > "$RESEARCH_PREFIX/installed.txt"
```

The linked Lean directory is the already hash-verified release, not a second
network installation. The elan 4.2.4 source was checked for `toolchain link`,
name-to-directory encoding, `--default-toolchain none`, and `--no-modify-path`.
Native loader compatibility still needs a runtime test. In particular, the
home-local utilities and ICU depend on trixie's libc, libgcc, libstdc++, and
compression libraries, and .NET needs the runtime image's OpenSSL/zlib stack.
Do not turn on invariant globalization to conceal a missing ICU installation.

## Workspace Setup

Setup executes in the main checkout after credentials become available, but
the commands below use only public repository/package sources. They do not
read, print, export, or copy a credential. They warm locked .NET packages, not a
shared Lean cache, and they do not create a second manual Worktree.

### Setup Command 1: Assert Inspected Source

<!-- cma-research:setup-01 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
test -s "$RESEARCH_PREFIX/installed.txt"
test "$(pwd -P)" = /home/cma/trureturing
test "$(git rev-parse HEAD)" = 5b9d526360dfb147036db8b0f663c408bd8cf33d
test -z "$(git status --porcelain --untracked-files=normal)"
test "$(tr -d '\n' < lean-toolchain)" = leanprover/lean4:v4.33.0
node <<'JS'
const fs = require('node:fs');
const sdk = JSON.parse(fs.readFileSync('global.json', 'utf8')).sdk;
const packages = JSON.parse(fs.readFileSync('lake-manifest.json', 'utf8')).packages;
const mathlib = packages.find(p => p.name === 'mathlib');
if (sdk.version !== '10.0.103' || sdk.allowPrerelease !== false) process.exit(1);
if (!mathlib || mathlib.inputRev !== 'v4.33.0' ||
    mathlib.rev !== 'db584cd6d46c92f209a44c0f1c829460d327499d') process.exit(1);
JS
test "$(dotnet --version)" = 10.0.103
```

### Setup Command 2: Locked .NET Restore

<!-- cma-research:setup-02 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
test "$(git rev-parse HEAD)" = 5b9d526360dfb147036db8b0f663c408bd8cf33d
dotnet restore tools/StrataLint.sln --locked-mode
```

### Setup Command 3: Check Entry Points And Clean Source

<!-- cma-research:setup-03 -->
```bash
set -euo pipefail
. /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh
test "$(git rev-parse HEAD)" = 5b9d526360dfb147036db8b0f663c408bd8cf33d
make help
make -C tools help
test ! -e .lake
test ! -L .lake
git diff --exit-code
git diff --cached --exit-code
test -z "$(git ls-files --others --exclude-standard)"
```

The later Agent must verify its own initial `HEAD` against this same assertion.
Successful Workspace setup is not evidence that the Agent's private .NET build,
Lean cache, inspector, admission, or first model answer has succeeded.

## Standing Prompt

Install this block verbatim as the Agent Profile's `system_prompt`. It directs
local research; publication and production authority must come from a separate
explicit user instruction.

<!-- cma-research:standing-prompt -->
```text
You are a mathematical research Agent working in your CMA-managed Worktree of
https://github.com/the-omega-institute/trureturing. Address the user's actual
research question with precise reasoning, inspectable local changes when
requested, and an honest account of what was proved and checked.

Before research edits, read AGENTS.md, all of CLAUDE.md without truncation, the
applicable nested instructions, README.md, and agents/CONTEXT.md. Read the
relevant parts of the sole normative specification,
docs/develop/spec/golden-ledger-repo-spec.md, and any applicable repository
skills. Do not treat this standing prompt as a replacement for those contracts.

On the first turn, confirm that git rev-parse --show-toplevel is your assigned
Worktree beneath /home/cma/trureturing/.worktrees, not the main checkout, and
that its initial HEAD is 5b9d526360dfb147036db8b0f663c408bd8cf33d. If the source
or toolchain differs, report the mismatch and stop before research edits; do
not reset, detach, or rewrite the managed branch to conceal it. On later turns,
preserve the user's work and your legitimate local commits instead of treating
every changed HEAD as a fresh-install failure.

Source /home/cma/.local/share/cma-research-lean4.33-dotnet10.0.103/env.sh in every
shell command that needs research tools. Confirm Lean 4.33.0, elan 4.2.4, and
.NET SDK 10.0.103. Keep the committed lake-manifest.json and .NET lockfiles.
Start with make help. Build the local harness with make -C tools dotnet. In
this actual Worktree, require that .lake is not a symlink and run make
lean-cache-ensure before Lean work. Never share, symlink, or copy a live .lake
between Worktrees. Do not run research in the Workspace's main checkout or
create an unmanaged second Worktree.

Follow the current research gates. D5 is the only M0 formal instance. Do not
promote the open hearts O-5/O-6 or open another theory root without its required
gate. Before creating an artifact, route a flat manifest with exactly theory,
plane, domain, module, generality, selector, artifact, and tag using StrataLint
route. Create only its returned canonical path and use the plane-specific
skeleton. Keep machine fields ASCII, module/type names CamelCase, theorem
names snake_case, and never hand-write status or fabricate a gate approval.

Choose a bounded next step that advances the user's question. State needed
assumptions and distinguish a conjecture, informal argument, local proof, and
admitted result. Do not introduce axioms, sorry/admit placeholders, weakened
statements, or bypasses as a substitute for proving the requested claim. Follow
the repository's stronger rules and report a genuine obstruction when found.

Use the pinned repository's Lean producer to create a source-bound report,
SHA-256 sidecar, and material archive, then run .NET admission separately with
--candidate-lean-report. In a clean candidate checkout pass the appropriate
reviewed --protected-base revision as well. Run make -C tools selftest
independently. A Lean build is not admission, and admission is not publication.
Record the exact source, commands, exit results, and evidence paths; never
present an unexecuted check or cached report for different inputs as success.

Work only within the user's authorized scope. This profile does not authorize
remote pushes, PRs, merges, release/cache publication, profile publication,
production changes, or additional service access. Do not print credentials or
put them into repository files. Use installed baseline skills only when their
task and authority apply; do not invent skill versions or service grants.
Finish each turn with the result, relevant local file references, validation
actually completed, and the next unresolved mathematical or technical issue.
```

For later authorized local verification, use the repository's documented
inspector and separate admission entry points. Run each in the Agent's actual
Worktree, source the environment in each shell, and preserve their logs and
report companions. The producer may replace its own specified output files,
so use a dedicated report path under that Worktree's `.lake/build/stratalint`.
Do not run a global cleanup, cache publication, or upstream PR workflow as part
of an environment smoke test.

## Publication And Journey Evidence

Publication remains a later authorized action. Before it, re-resolve the public
default branch and fail closed on drift, confirm the chosen model route and
capacity bounds, verify platform policy, and establish the caller's durable
delegation prerequisite. A bearer-only request is not proof that renewable
Sandbox credentials can be issued. Do not paste a credential into a profile.

1. Create the assembled Agent draft with `POST /api/v1/agent-profiles`; publish
   its current `draft_version` through `POST /api/v1/agent-profiles/{id}/revisions`.
   Read the returned immutable revision and record its real ID, revision, digest,
   owner, and publication time.
2. Insert that exact Agent reference into the Workspace draft. Create and
   publish via the corresponding `/api/v1/workspace-profiles` routes, then
   read back and record its immutable revision.
3. Insert the exact Workspace reference into the Sandbox draft. Create and
   publish via the corresponding `/api/v1/sandbox-profiles` routes, then read
   back the complete nested content and record its immutable revision.
4. Set intended public visibility explicitly through each profile's visibility
   route after reviewing the complete public content. A draft save, public
   visibility, and immutable publication are distinct operations.
5. For the second-owner journey, sign up or sign in as a genuinely different
   owner, open the published **Sandbox Profile**, and fork that Sandbox Profile.
   Verify the new owner and `forked_from`; do not substitute an Agent-only fork.
   Inspect inherited published references and review the server-provided trust
   disclosure and commands digest. Ownership of the fork does not justify
   silently skipping trust for foreign nested commands.
6. From the second owner's fork, explicitly admit a new Sandbox and research
   Agent. Observe install, credential activation, Workspace setup, private
   Worktree creation, and the Agent's first durable user message and real answer.
   Fresh-signup coverage must use a genuinely fresh account; an existing-owner
   browser test is separate evidence. Navigation or an OAuth callback alone
   must not admit work.

Record actual profile references and instance/operation IDs only after those
steps occur. No publication IDs, signup results, trust approvals, first-answer
transcripts, or deployed image digests exist in this preparation record.

### Evidence Status

| Check | Status in this preparation |
|---|---|
| Current default branch and observed commit | Read-only Git observation recorded above; it is not a promise that the branch will remain there. |
| Eighteen source contracts at old and refreshed commits | Bytes compared; all unchanged. Full upstream `CLAUDE.md` reading is required of the research Agent, not claimed here. |
| Six Linux x64 artifact hashes | Downloaded bytes matched the pinned hashes in the install commands. Lean/elan hashes came from GitHub release metadata, .NET from Microsoft release metadata, Debian packages from the HTTPS trixie amd64 package index. Independent Debian release-signature verification was not performed. |
| Archive entry inspection | Expected Lean/lake, elan-init, .NET SDK, make, ICU, and zstd files exist. Entry names contained no absolute or parent-traversal paths. This is not native execution or a full archive-security audit. |
| Recipe syntax and profile structure | Passed: eight install and three setup commands all parse with `bash -n`; maximum command size 1,238 bytes against the 4,096-byte limit; standing prompt 3,760 bytes against 32 KiB. Three blueprint assemblies passed JSON round-trip, field/enum, environment-sourcing, and commit-assertion checks. ASCII, whitespace, embedded JavaScript parsing, and strict gitleaks scanning passed. These checks are not Core admission or publication. |
| Runtime image and native dependencies | Pending. Docker was unhealthy and was not retried for this task; no container or deployed Sandbox smoke test was run. |
| Install and setup as UID/GID 10001 | Pending, including readonly-root behavior, loader/ICU compatibility, network timeouts, storage use, and exact recorded source commit. |
| Real Agent Worktree | Pending: verify its initial base, private `.lake`, locked harness build, mathlib cache, Lean producer, separate admission, and selftests. |
| Publication and second-owner fork | Pending; no external writes were performed. |
| Fresh signup and real first answer | Pending; neither mocked browser state nor source review establishes this result. |

The archive entry counts inspected locally were Lean 18,130; .NET 5,799; elan
1; make 118; ICU 24; zstd 30. The complete actual-byte checks must be repeated
by the install commands in the target Sandbox. Artifact availability, static
correctness, and a successful user journey are separate claims.
