# Security policy

The maintainers for private vulnerability reports are `@chronoai-shining` and the
repository owner. This policy covers the first source-only OSS release.

## Report privately

Do not disclose suspected vulnerabilities, exploit details, credentials or
private data in public issues, pull requests, discussions or logs.

Use [GitHub private vulnerability reporting](https://github.com/ChronoAIProject/cma/security/advisories/new).
Do not disclose suspected vulnerabilities, exploit details, credentials or
private data in public issues, pull requests, discussions or logs. There is no
email fallback; if the private GitHub channel is unavailable, withhold technical
details and contact a maintainer through an existing private GitHub channel.

Once a private channel is verified, include the affected version or commit,
affected component, expected and observed behavior, a minimal synthetic
reproduction, likely impact and any proposed mitigation. Share only information
necessary to reproduce the problem; redact credentials and unrelated data.
Testing must be limited to systems and data you are authorized to examine.

## Triage and coordinated disclosure

- Security responsibility: `@chronoai-shining` and the repository owner.
- Acknowledgement target: No response-time SLA is promised by this source-only
  release; maintainers will acknowledge reports when they can access them.
- Progress update cadence: Maintainers will use the private report thread for
  material updates when investigation continues.
- Disclosure process: Maintainers coordinate remediation and advisory timing
  through the private GitHub report. Do not publish unpatched details. Reporter
  credit is handled according to the reporter's preference where practical.

Share reports only with people needed for investigation and remediation. Agree on
disclosure timing through the private channel and avoid publishing unpatched
details before coordination.

## Supported versions and release scope

The first OSS release is source-only. No binary, image or hosted-service support
matrix is asserted by this policy. Hosted service operations and future packaged
distributions require a separately published support statement.

Reference: [GitHub's private reporting instructions](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately).
