# Validate cache authority and the independently scheduled Web jobs.

def check_component_cache_writers(documents)
  develop = "github.event_name == 'push' && github.ref == 'refs/heads/develop'"
  writer = "${{ #{develop} }}"
  image_job = documents.fetch('ci-images').fetch('jobs').fetch('images')
  group = "${{ #{develop} && format('cma-develop-ci-image-{0}', matrix.component) || " \
    "format('ci-image-{0}-{1}-{2}', github.run_id, github.run_attempt, matrix.component) }}"
  require_check(image_job['concurrency'] == {'group' => group, 'queue' => 'max', 'cancel-in-progress' => false},
    'develop image jobs must queue per component without cancelling active writers')
  images = image_job.fetch('steps')
  node_setup = ->(s) { s.fetch('uses', '').start_with?('actions/setup-node@') }
  image_nodes = images.select(&node_setup)
  require_check(!image_nodes.empty? && image_nodes.all? { |s|
    s.dig('with', 'package-manager-cache') == false && !s.fetch('with').key?('cache')
  }, 'image Node setup must disable automatic and explicit dependency caching')
  scope = "matrix.component == 'api' && 'cma-am' || matrix.component == 'cmaeg' && 'cma-eg' || matrix.component == 'web' && 'cma-web' || matrix.component"
  builds = images.select { |s| s.fetch('uses', '').start_with?('docker/build-push-action@') }
  bounded = "${{ steps.layer-validate.outcome == 'success' && format('type=local,src={0}/cma-layer-cache/restored', runner.temp) || '' }}"
  require_check(builds.size == 2 && builds.all? { |s| s.dig('with', 'cache-from').to_s.lines.map(&:strip) ==
    ["type=gha,version=2,scope=${{ #{scope} }}", bounded] }, 'both image builds must restore validated component caches')
  exports = builds.map { |s| s.dig('with', 'cache-to') }.compact
  require_check(exports.empty?, 'CI image builds must not publish unbounded layers')
  {'docker/setup-qemu-action' => 'cache-image', 'docker/setup-buildx-action' => 'cache-binary',
   'aquasecurity/trivy-action' => 'cache'}.each do |action, option|
    step = images.find { |s| s.fetch('uses', '').start_with?(action + '@') }
    require_check(step&.dig('with', option) == writer, "#{action} may save a PR cache")
  end
  web_jobs = documents.fetch('ci-web').fetch('jobs').values_at(*WEB_JOBS).map { |job| job.fetch('steps') }
  require_check(web_jobs.all? { |steps|
    web_nodes = steps.select(&node_setup)
    web_nodes.size == 1 && web_nodes.first.dig('with', 'package-manager-cache') == false &&
      web_nodes.first.dig('with', 'cache') == "${{ #{develop} && 'npm' || '' }}"
  }, 'setup-node may save a PR cache')
  writers = (images + web_jobs.flatten(1)).select { |s| s.fetch('uses', '').match?(%r{\Aactions/cache(?:/save)?@}) }
  require_check(writers.size == 1 && writers.first['uses'].start_with?('actions/cache/save@') &&
    writers.first['if'] == "#{STAGING_IMAGE_FALLBACK} && (success() && #{develop} && matrix.component == 'api' && vars.CMA_LAYER_CACHE_WRITER_FLOOR != 'true' && steps.api-cargo-cache-export.outputs.ready == 'true')",
    'only the accepted develop Cargo archive may be saved')
  restore = 'actions/cache/restore@caa296126883cff596d87d8935842f9db880ef25'
  npm = {'path' => '${{ steps.npm-cache.outputs.directory }}',
    'key' => "node-cache-${{ runner.os }}-${{ steps.npm-cache.outputs.architecture }}-npm-${{ hashFiles('web/package-lock.json') }}"}
  database = {'path' => '${{ github.workspace }}/.cache/trivy', 'key' => 'cache-trivy-readonly', 'restore-keys' => 'cache-trivy-'}
  restores = web_jobs.map { |steps| [steps, 'Restore npm download cache', 'Install (lockfile integrity)', npm] }
  restores << [images, 'Restore Trivy databases', 'Vulnerability scan', database]
  restores.each do |steps, name, consumer, inputs|
    index = steps.index { |s| s['name'] == name }
    run = steps.index { |s| s['name'] == consumer }
    require_check(index && run && index < run && steps[index]['uses'] == restore && steps[index]['with'] == inputs &&
      steps[index]['if'] == "github.event_name != 'push' || github.ref != 'refs/heads/develop'",
      "#{name} must restore without saving before #{consumer}")
  end
end

# The Web track runs as parallel jobs. Each opens with the same reviewed setup
# and keeps the track's runner, directory and bound; the bundle build and the
# pinned browser are repeated, identically, only in jobs that consume them, and
# every other step runs in exactly one job.
WEB_SETUP = 5
WEB_PRODUCERS = ['Production build', 'Install pinned local browser'].freeze

WEB_JOBS = %w[web web-journeys web-lifecycle web-design web-conversation].freeze

# CI / Web sees only the reusable workflow's aggregate result. The always()
# web-result job must need exactly the named Web jobs and require success.
def check_web_result(documents)
  all = documents.fetch('ci-web').fetch('jobs')
  require_check(all.keys.sort == (WEB_JOBS + ['web-result']).sort, 'Web jobs differ from the named Web track')
  result = all.fetch('web-result')
  require_check(result['if'] == 'always()' && result['needs'] == WEB_JOBS &&
    result['timeout-minutes'] == 5 && result['runs-on'] == 'ubuntu-latest', 'web-result must always run and need every Web job')
  gate = {
    'name' => 'Require every Web job to succeed',
    'env' => {'NEEDS' => '${{ toJSON(needs) }}'},
    'run' => 'python3 -B .github/scripts/ci_tracks/web_result.py'
  }
  require_check(result.fetch('steps') == [all.fetch('web').fetch('steps').first, gate] &&
    !result.key?('continue-on-error'), 'web-result must check every named job result')
  listed = File.read(File.join(__dir__, 'ci_tracks', 'web_result.py'))[/^JOBS = \((.*?)\)$/, 1].to_s.scan(/"([^"]+)"/).flatten
  require_check(listed == WEB_JOBS, 'web_result.py must list the named Web jobs')
end

def check_web_jobs(documents)
  jobs = documents.fetch('ci-web').fetch('jobs').values_at(*WEB_JOBS)
  setup = jobs.first.fetch('steps').take(WEB_SETUP)
  require_check(setup.last&.fetch('name', nil) == 'Install (lockfile integrity)', 'Web setup prefix differs')
  require_check(jobs.all? { |job|
    job.fetch('steps').take(WEB_SETUP) == setup && job['runs-on'] == 'ubuntu-latest' &&
      job['defaults'] == {'run' => {'working-directory' => 'web'}} && job['timeout-minutes'] == 60 &&
      (job.keys - %w[name runs-on defaults steps timeout-minutes]).empty?
  }, 'Web job setup, runner, directory or bound differs')
  names = jobs.flat_map { |job| job.fetch('steps').drop(WEB_SETUP).map { |s| s['name'] } }
  owned = names - WEB_PRODUCERS
  require_check(names.none?(&:nil?) && owned.uniq.size == owned.size, 'a Web step must run in exactly one job')
  browser_jobs = jobs.drop(1)
  WEB_PRODUCERS.each do |producer|
    copies = browser_jobs.map { |job| job.fetch('steps').select { |s| s['name'] == producer } }
    require_check(jobs.first.fetch('steps').none? { |s| s['name'] == producer } &&
      copies.all? { |found| found.size == 1 } && copies.flatten(1).uniq.size == 1,
      "#{producer} must be identical in each browser job")
  end
end
