# Contributing

Read `CLAUDE.md` for architecture, compatibility and review requirements. Link a
focused pull request to its tracking issue and explain the behavior and checks.
Run the relevant Rust checks from `api/` and web checks from `web/` using the
pinned toolchains and lockfiles. Run the public repository boundary regression
suite with `ruby .github/scripts/check-public-tree-tests.rb`.

## Public repository boundary

Never commit infrastructure or deployment manifests, Kubernetes or Helm YAML,
cloud project, cluster, IAM or service-account identifiers, internal hostnames
or IPs, Terraform state, or environment-specific configuration. Infrastructure
lives outside GitHub. Internal documentation, runbooks, verification records,
notes, plans and investigations are not committed to this public repository.
Do not add `docs/`, `infra/`, or `sdk/`, or copy private material elsewhere in the
tree. Coordinate operational changes through private maintainer channels.

The three public Event Gateway guides are code-owned application assets. Update
their scrubbed examples and contract tests together. Keep environment identifiers
and credentials out of examples, test fixtures, issues and pull request text.
