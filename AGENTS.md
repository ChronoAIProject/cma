# Contributor and agent instructions

Read `CLAUDE.md` before changing code. Preserve application behavior and run the
relevant local checks before submitting a pull request.

## Public repository boundary

Never commit infrastructure or deployment manifests, Kubernetes or Helm YAML,
cloud project, cluster, IAM or service-account identifiers, internal hostnames
or IPs, Terraform state, or environment-specific configuration. Infrastructure
lives outside GitHub. Internal documentation, runbooks, verification records,
notes, plans and investigations are not committed to this public repository.
Do not write into `docs/`, `infra/`, or `sdk/`, or move private material to another
tracked path. Keep private references outside the repository.

Only the three reviewed public Event Gateway guides are retained as application
assets under `api/crates/cmaeg/assets/docs/public/`. Build recipes live beside
their owning components at the five paths listed in `CLAUDE.md`. Keep tests
synthetic and free of credentials or real environment identifiers.
