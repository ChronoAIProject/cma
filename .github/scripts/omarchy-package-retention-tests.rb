# frozen_string_literal: true

require "minitest/autorun"
require "fileutils"
require "open3"
require "tmpdir"
require "yaml"
require_relative "omarchy-package-retention"

class OmarchyPackageRetentionTests < Minitest::Test
  ROOT = File.expand_path("../..", __dir__)
  BUNDLE = ".github/workflows/omarchy-package-bundle.yml"
  SWEEP = ".github/scripts/actions_cache_sweep.py"
  API = "https://api.github.com/orgs/chronoaiproject"

  def real(path)
    File.read(File.join(ROOT, path), encoding: "UTF-8")
  end

  # The real bundle workflow with one more step: the most likely place for a
  # cleanup to be added.
  def bundle_with(step)
    workflow = YAML.safe_load(real(BUNDLE), aliases: true)
    workflow["jobs"]["bundle"]["steps"] << step
    YAML.dump(workflow)
  end

  def refused(path, content, label)
    refute_empty OmarchyPackageRetention.violations(path, content), label
  end

  def permitted(path, content, label)
    assert_empty OmarchyPackageRetention.violations(path, content), label
  end

  def test_current_repository_passes
    assert_empty OmarchyPackageRetention.check(ROOT)
    permitted(BUNDLE, real(BUNDLE), "the publisher names the package without deleting it")
    permitted(SWEEP, real(SWEEP), "Actions cache deletes are unrelated")
  end

  def test_rest_deletes_of_the_bundle_or_a_computed_package_are_refused
    {
      "curl version" => "curl -X DELETE #{API}/packages/container/cma-omarchy-packages/versions/42",
      "curl package" => "curl --request DELETE #{API}/packages/container/cma-omarchy-packages",
      "user package" => "curl -XDELETE https://api.github.com/user/packages/container/CMA-Omarchy-Packages",
      "Python literal" => %(Request(f"#{API}/packages/container/cma-omarchy-packages/versions/{vid}", method="DELETE")),
      "Python computed" => %(Request(f"orgs/{org}/packages/container/{name}/versions/{vid}", method="DELETE")),
      "concatenated" => %(fetch("/user/packages/container/" + name, { method: "DELETE" })),
      "every package" => %(gh api "orgs/x/packages?package_type=container" --jq '.[].url' | xargs -n1 gh api -X DELETE),
    }.each do |label, command|
      refused(".github/scripts/cleanup.sh", command, label)
      refused(SWEEP, real(SWEEP) + "\n# #{command}\n", "#{label} beside the cache sweep")
    end
  end

  def test_gh_api_deletes_are_refused
    path = "/orgs/chronoaiproject/packages/container/cma-omarchy-packages"
    ["gh api -X DELETE #{path}/versions/1", "gh api --method DELETE \"#{path}\"", "gh api --method=DELETE #{path}",
     "gh api -XDELETE #{path}/versions/1", "gh api -X delete #{path}", "gh api --method 'delete' #{path}/versions/1",
     'gh api -X DELETE "/orgs/$OWNER/packages/container/$PACKAGE/versions/$ID"',
     "gh api -X DELETE /orgs/x/packages/${{ inputs.type }}/${{ inputs.name }}"].each do |command|
      refused(BUNDLE, bundle_with("name" => "Prune", "run" => command), command)
      refused(".github/actions/prune/action.yml", YAML.dump("runs" => { "using" => "composite",
        "steps" => [{ "shell" => "bash", "run" => command }] }), "composite #{command}")
    end
  end

  def test_octokit_package_deletions_are_refused
    ["deletePackageVersionForOrg({ package_type: 'container', package_name: 'cma-omarchy-packages', org, package_version_id })",
     "deletePackageForOrg({ package_type: 'container', package_name: name, org })",
     "deletePackageVersionForAuthenticatedUser({ package_type, package_name, package_version_id })"].each do |call|
      step = { "uses" => "actions/github-script@" + "a" * 40, "with" => { "script" => "await github.rest.packages.#{call}" } }
      refused(BUNDLE, bundle_with(step), call)
    end
  end

  def test_ruby_net_http_deletes_are_refused
    {
      "request class" => %(Net::HTTP::Delete.new(URI("#{API}/packages/container/cma-omarchy-packages/versions/1"))),
      "computed name" => %(request = Net::HTTP::Delete.new("/orgs/x/packages/container/\#{name}/versions/\#{id}")),
      "literal path" => %(http.delete("/orgs/chronoaiproject/packages/container/cma-omarchy-packages")),
      "client session" => %(require "net/http"\npath = "/user/packages/container/\#{name}"\n) +
        %(Net::HTTP.start("api.github.com", 443, use_ssl: true) { |http| http.delete(path, headers) }),
    }.each { |label, source| refused(".github/scripts/prune.rb", source, label) }
  end

  def test_python_client_deletes_are_refused
    {
      "requests helper" => %(requests.delete(f"#{API}/packages/container/cma-omarchy-packages/versions/{vid}")),
      "httpx helper" => %(httpx.delete(f"{API}/orgs/{org}/packages/container/{name}")),
      "requests method" => %(requests.request("delete", f"#{API}/packages/container/cma-omarchy-packages")),
      "requests session" => %(import requests\nsession = requests.Session()\n) +
        %(url = f"{API}/user/packages/container/{name}"\nsession.delete(url)),
      "httpx client" => %(import httpx\nwith httpx.Client(base_url=API) as client:\n) +
        %(    client.delete(f"/orgs/x/packages/container/{name}")),
    }.each do |label, source|
      refused(".github/scripts/prune.py", source, label)
      refused(SWEEP, real(SWEEP) + "\n#{source}\n", "#{label} beside the cache sweep")
    end
  end

  def test_any_case_fetch_methods_are_refused
    {
      "lowercase" => %(await fetch(`/orgs/x/packages/container/${name}`, { method: 'delete' })),
      "capitalised" => %(await fetch("#{API}/packages/container/cma-omarchy-packages", { method: "Delete" })),
      "mixed" => %(const url = "/user/packages/npm/" + pkg;\nawait fetch(url, {method:`dElEtE`})),
      "axios helper" => %(await axios.delete(`/orgs/x/packages/container/${name}`)),
      "axios config" => %(await axios({ url: `/user/packages/container/${name}`, method: 'delete' })),
      "urllib lowercase" => %(Request(f"{API}/orgs/x/packages/container/{name}", method="delete")),
    }.each do |label, source|
      refused(".github/scripts/prune.mjs", source, label)
      step = { "uses" => "actions/github-script@" + "a" * 40, "with" => { "script" => source } }
      refused(BUNDLE, bundle_with(step), "#{label} in github-script")
    end
  end

  def test_field_listings_driving_deletes_are_refused
    each_url = "--jq '.[].url' | xargs -n1 gh api -X DELETE"
    ["gh api --method GET /orgs/x/packages -f package_type=container #{each_url}",
     "gh api --method GET /orgs/x/packages -F package_type=container #{each_url}",
     "gh api --method GET \"/users/$OWNER/packages\" --field package_type=docker #{each_url}",
     "gh api --method GET --raw-field \"package_type=container\" user/packages #{each_url}"].each do |command|
      refused(BUNDLE, bundle_with("name" => "Prune", "run" => command), command)
      refused(".github/scripts/prune.sh", command, "script #{command}")
    end
    refused(".github/scripts/prune.py", %(rows = requests.get("#{API}/packages", params={"package_type": "container"}).json()\n) +
      %(for row in rows:\n    requests.delete(row["url"])), "requests listing")
    refused(".github/scripts/prune.mjs", <<~JS, "Octokit listing")
      const all = await github.paginate("GET /orgs/{org}/packages", { org, package_type: "container" });
      for (const p of all) await github.request("DELETE " + p.url);
    JS
  end

  def test_registry_client_deletes_are_refused
    image = "ghcr.io/chronoaiproject/cma-omarchy-packages"
    ["skopeo delete docker://#{image}:2026-10-22-abc",
     "skopeo delete --authfile /auth/config.json \"docker://ghcr.io/$REPOSITORY:$TAG\"",
     "skopeo --insecure-policy delete --creds \"$USER:$TOKEN\" docker://#{image}@sha256:#{'c' * 64}",
     "skopeo delete docker://ghcr.io/${{ env.REPOSITORY }}:old",
     "skopeo delete \"$IMAGE\" || true",
     "skopeo list-tags docker://#{image} | jq -r '.Tags[]' | xargs -I{} skopeo delete docker://#{image}:{}",
     "crane delete #{image}@sha256:#{'d' * 64}",
     "gcrane delete \"$REFERENCE\"",
     "oras manifest delete --force #{image}:old",
     "regctl tag delete #{image}:old"].each do |command|
      refused(BUNDLE, bundle_with("name" => "Prune", "run" => command), command)
      refused(".github/scripts/prune.sh", command, "script #{command}")
    end
    refused(".github/scripts/prune.sh", "skopeo delete \\\n  docker://#{image}:old\n", "continued line")
    refused(".github/scripts/prune.py", %(subprocess.run(["skopeo", "delete", f"docker://ghcr.io/{repository}:{tag}"])),
      "argument list")
    refused(".github/scripts/prune.py", %(command = ["skopeo", "delete"]\ncommand.append(reference)), "reference from elsewhere")
  end

  def test_cleanup_actions_pointed_at_the_bundle_every_package_or_a_computed_name_are_refused
    pin = "@" + "b" * 40
    [["actions/delete-package-versions#{pin}", { "package-name" => "cma-omarchy-packages", "package-type" => "container" }],
     ["actions/delete-package-versions#{pin}", { "package-name" => "${{ matrix.package }}" }],
     ["actions/delete-package-versions@v5", nil],
     ["snok/container-retention-policy#{pin}", { "image-names" => "cma-omarchy-packages" }],
     ["snok/container-retention-policy@v3", { "image-names" => "cma-runtime, cma-omarchy-packages" }],
     ["snok/container-retention-policy@v3", { "image-names" => "*" }],
     ["snok/container-retention-policy@v3", { "image-names" => "cma-*" }],
     ["Snok/Container-Retention-Policy@v2", { "account" => "chronoaiproject" }],
     ["docker://ghcr.io/snok/container-retention-policy:v3", { "image-names" => "cma-omarchy-*" }],
     ["dataaxiom/ghcr-cleanup-action@v1", { "packages" => "cma-omarchy-packages" }],
     ["vlaurin/action-ghcr-prune@v0.6.0", {}],
     ["bots-house/ghcr-delete-image-action@v1.1.0", { "name" => "cma-omarchy-packages" }]].each do |uses, inputs|
      step = { "uses" => uses }
      step["with"] = inputs if inputs
      refused(BUNDLE, bundle_with(step), "#{uses} #{inputs.inspect}")
      job = { "runs-on" => "ubuntu-24.04", "steps" => [step] }
      refused(".github/workflows/retention.yaml", YAML.dump("on" => "schedule", "jobs" => { "prune" => job }), uses)
      refused(".github/actions/prune/action.yaml", YAML.dump("runs" => { "using" => "composite", "steps" => [step] }), uses)
    end
  end

  def test_unrelated_deletes_and_other_packages_are_permitted
    permitted(".github/scripts/cleanup.sh", "gh api --method DELETE repos/x/actions/caches/1", "Actions cache")
    permitted(".github/scripts/cleanup.sh", "curl -X DELETE #{API}/packages/container/cma-ci-scratch/versions/1", "other package")
    permitted(".github/plan-release.sh", real(".github/plan-release.sh") + "\ncurl -X DELETE \"$URL\"\n", "package page link")
    permitted(".github/scripts/notes.md", "packages/container/cma-omarchy-packages/versions is listed only", "read-only path")
    permitted(".github/scripts/data.json", "usr/share/mime/packages/freedesktop.org.xml DELETE", "filesystem path")
    permitted(BUNDLE, bundle_with("uses" => "actions/delete-package-versions@v5",
      "with" => { "package-name" => "cma-ci-scratch" }), "cleanup of another package")
    permitted(BUNDLE, bundle_with("uses" => "snok/container-retention-policy@v3",
      "with" => { "image-names" => "cma-ci-* cma-scratch" }), "retention of other packages")
    permitted(BUNDLE, real(BUNDLE) + "\n# uses: actions/delete-package-versions@v5\n", "comment is not a step")
    permitted(".github/scripts/example.yml", YAML.dump("steps" => [{ "uses" => "snok/container-retention-policy@v3" }]),
      "YAML outside workflows and actions never runs")
  end

  def test_client_deletes_of_other_endpoints_and_registry_reads_are_permitted
    {
      "Actions cache through requests" => %(requests.delete(f"https://api.github.com/repos/x/actions/caches/{cache_id}")),
      "Agent through an httpx client" => %(import httpx\nclient = httpx.Client()\nclient.delete(f"{CMA}/api/agents/{agent_id}")),
      "Sandbox through fetch" => %(await fetch(`${base}/api/sandboxes/${id}`, { method: "delete" })),
      "Workspace through Net::HTTP" => %(Net::HTTP::Delete.new(URI("https://cma.example/api/workspaces/\#{id}"))),
      "another package" => %(requests.delete(f"#{API}/packages/container/cma-ci-scratch/versions/{vid}")),
      "Set delete beside a read" => %(const versions = await fetch(`/orgs/x/packages/container/${name}/versions`);\n) +
        %(seen.delete(name);),
      "Hash delete beside a read" => %(versions = get("/orgs/x/packages/container/\#{name}/versions")\n) +
        %(headers.delete("Authorization")),
      "listing without a delete" => "gh api /orgs/x/packages -f package_type=container --jq '.[].name'",
      "packages page beside a cache delete" => %(curl -X DELETE "$API/repos/x/actions/caches/1"\n) +
        "# Packages: https://github.com/orgs/chronoaiproject/packages\n",
      "registry delete of another package" => "skopeo delete --authfile \"$AUTH\" --creds=\"$USER:$TOKEN\" " \
        "docker://ghcr.io/chronoaiproject/cma-ci-scratch:pr-1",
      "crane delete of another package" => "crane delete ghcr.io/chronoaiproject/cma-ci-scratch@sha256:#{'e' * 64}",
      "continued delete of another package" => "skopeo delete \\\n  docker://ghcr.io/chronoaiproject/cma-ci-scratch:pr-1\n",
      "registry reads" => %(skopeo copy "docker://ghcr.io/$REPOSITORY:$TAG" oci:/tmp/layout\n) +
        %(skopeo inspect --raw "docker://ghcr.io/$REPOSITORY:$TAG"\ncrane digest "$IMAGE"),
    }.each { |label, source| permitted(".github/scripts/cleanup.sh", source, label) }
  end

  def test_unreadable_workflow_is_refused
    refused(".github/workflows/broken.yml", "jobs: [", "invalid YAML")
  end

  def test_only_the_contract_and_its_tests_are_exempt
    own = [File.join(__dir__, "omarchy-package-retention.rb"), File.join(__dir__, File.basename(__FILE__))]
    assert_equal own.map { |file| file.delete_prefix("#{ROOT}/") }.sort, OmarchyPackageRetention::EXEMPT.sort
    command = "gh api -X DELETE /orgs/x/packages/container/cma-omarchy-packages"
    OmarchyPackageRetention::EXEMPT.each { |path| permitted(path, command, path) }
    refused(".github/scripts/omarchy-package-retention-helper.rb", command, "a neighbouring file")
  end

  def test_tracked_files_are_checked
    Dir.mktmpdir("omarchy-retention-") do |root|
      _out, error, status = Open3.capture3("git", "init", "--quiet", root)
      assert status.success?, error
      FileUtils.mkdir_p(File.join(root, ".github/workflows"))
      File.write(File.join(root, ".github/workflows/prune.yml"),
        YAML.dump("jobs" => { "prune" => { "steps" => [{ "uses" => "snok/container-retention-policy@v3" }] } }))
      assert_empty OmarchyPackageRetention.check(root), "untracked files are not part of the tree"
      _out, error, status = Open3.capture3("git", "-C", root, "add", ".github/workflows/prune.yml")
      assert status.success?, error
      assert_equal [[".github/workflows/prune.yml", "snok/container-retention-policy@v3 can delete versions of cma-omarchy-packages"]],
        OmarchyPackageRetention.check(root)
    end
  end
end
