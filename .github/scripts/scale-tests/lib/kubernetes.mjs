import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { check, hash } from './checks.mjs';
import { localUrl } from './safety.mjs';

const execute = promisify(execFile);
const allowedResources = new Set(['pods', 'deployments', 'horizontalpodautoscalers', 'poddisruptionbudgets']);

export class LocalCluster {
  constructor({ kubeconfig, context, namespace, namespaceHash }, command = execute) {
    check(typeof kubeconfig === 'string' && kubeconfig.startsWith('/'), 'explicit_kubeconfig_required', true);
    check(context === 'kind-cma-scale' && namespace === 'cma-scale', 'cluster_scope_refused', true);
    this.args = ['--kubeconfig', kubeconfig, '--context', context, '--namespace', namespace, '--request-timeout=10s'];
    this.namespaceHash = namespaceHash;
    this.command = command;
    this.verified = false;
  }

  async invoke(args) {
    const { stdout } = await this.command('kubectl', [...this.args, ...args],
      { timeout: 15000, maxBuffer: 4 * 1024 * 1024, encoding: 'utf8' });
    return stdout;
  }

  async verify() {
    this.verified = false;
    // Read only the API-server address, never credential values or Secret objects.
    const server = await this.invoke(['config', 'view', '--minify', '-o', 'jsonpath={.clusters[0].cluster.server}']);
    localUrl(server.trim());
    const namespace = JSON.parse(await this.invoke(['get', 'namespace', 'cma-scale', '-o', 'json']));
    check(namespace.metadata?.name === 'cma-scale' && hash(namespace.metadata.uid) === this.namespaceHash,
      'namespace_identity_changed', true);
    this.verified = true;
  }

  async read(resource) {
    check(allowedResources.has(resource), 'kubernetes_resource_refused');
    await this.verify();
    return JSON.parse(await this.invoke(['get', resource, '-o', 'json']));
  }

  async removePod({ name, uidHash, labels }) {
    check(typeof name === 'string' && /^[a-z0-9][a-z0-9.-]{0,252}$/.test(name), 'invalid_pod_name');
    check(labels?.['app.kubernetes.io/part-of'] === 'cma-scale-tests', 'pod_not_test_owned');
    const pods = await this.read('pods');
    const pod = pods.items.find((item) => item.metadata.name === name);
    check(pod && hash(pod.metadata.uid) === uidHash
      && pod.metadata.labels?.['app.kubernetes.io/part-of'] === 'cma-scale-tests', 'pod_identity_changed');
    // API deletion is only an injection request. The driver must separately prove process death.
    await this.invoke(['delete', 'pod', name, '--grace-period=0', '--force', '--wait=false']);
    return { pod_hash: uidHash, processDeathObserved: false };
  }
}
