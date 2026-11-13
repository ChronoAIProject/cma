#!/usr/bin/env python3
"""Control plane topology. Runtime/identity fixtures are a separate required gate."""
import json
from pathlib import Path
import re
import sys

ROOT=Path(__file__).resolve().parent.parent
NS='cma-scale'
image=sys.argv[1]
sha=sys.argv[2]
if not re.fullmatch(r'[0-9a-f]{40}',sha): raise SystemExit('Expected full source SHA')
runtime_source=(ROOT/'source/infra/api/configmap.yaml').read_text()
runtime_image=re.search(r'^  CHRONO_SANDBOX_IMAGE: "([^"]+)"',runtime_source,re.M).group(1)
def obj(kind,name,spec=None,api='v1',**kw):
    o={'apiVersion':api,'kind':kind,'metadata':{'name':name,'namespace':NS},**kw}
    if spec is not None: o['spec']=spec
    return o
common={'CHRONO_SERVER_PUBLIC_URL':'https://localhost:18080','CHRONO_SERVER_CORS_ORIGIN':'https://localhost:18080',
    'CHRONO_SERVER_BIND':'0.0.0.0:8080','CHRONO_AUTH_MODE':'provider',
    'CHRONO_NYXID_BASE_URL':'https://identity.cma-scale.invalid',
    'CHRONO_NYXID_PUBLIC_URL':'https://identity.cma-scale.invalid','CHRONO_NYXID_CLIENT_ID':'local-scale-am',
    'CHRONO_MONGODB_DATABASE':'cma_scale','CHRONO_MONGODB_AUTHORITY_MODE':'shared-schema',
    'CHRONO_DEPLOYMENT_ID':'cma-scale-local','CHRONO_SANDBOX_PROVIDER':'agent-sandbox',
    'CHRONO_RETAIN_OPENSANDBOX':'false','CHRONO_AGENT_SANDBOX_ENABLED':'true',
    'CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE':'/run/cma-local/sandbox-key',
    'CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT':'direct','CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT':'cma-scale-runtime',
    'CHRONO_AGENT_SANDBOX_HOME_MODE':'native_vct_full_home_v2',
    'CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS':'standard','CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB':'1',
    'CHRONO_SANDBOX_NAMESPACE':NS,'CHRONO_SANDBOX_IMAGE':runtime_image,
    'CHRONO_NATIVE_PREPARED_TARGET':'0','CHRONO_NATIVE_MACHINE_PREPARED_TARGET':'0',
    'CHRONO_NATIVE_WARM_POOL_TARGET':'0','CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET':'0',
    'CHRONO_UPSTREAM_BASE_URL':'https://provider.cma-scale.invalid',
    'CHRONO_UPSTREAM_WIRE_API':'responses','CHRONO_PLATFORM_BASE_URL':'https://identity.cma-scale.invalid',
    'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED':'true','CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS':'256',
    'CHRONO_CMAEG_BASE_URL':'http://cma-event-gateway.cma-scale.svc',
    'CHRONO_CMAEG_PUBLIC_URL':'https://localhost:18080',
    'CHRONO_V2_UI_TRANSPORT_ENABLED':'true','RUST_LOG':'warn'}
gateway={'CMAEG_LISTEN':'0.0.0.0:8090','CMAEG_METRICS_LISTEN':'0.0.0.0:9091',
    'CMAEG_PUBLIC_URL':'https://localhost:18080','CMAEG_PUBLIC_API_URL':'https://localhost:18080/event-gateway',
    'CMAEG_CMA_BASE_URL':'http://cma-agent-manager.cma-scale.svc',
    'CMAEG_NYXID_BASE_URL':'https://identity.cma-scale.invalid','CMAEG_NYXID_CLIENT_ID':'local-scale',
    'CMAEG_MONGODB_DATABASE':'cma_scale','CMAEG_MONGODB_AUTHORITY_MODE':'shared-schema',
    'CMAEG_CONSOLE_ENABLED':'false','CMAEG_PROVIDERS':'[]','CMAEG_MEDIA_FFMPEG_DIR':'',
    'CMAEG_MULTI_CONSUMER_WRITES_ENABLED':'true','CMAEG_THREAD_CONTRACT_WRITES_ENABLED':'true',
    'RUST_LOG':'warn'}
items=[obj('ConfigMap','am-local',data=common),obj('ConfigMap','eg-local',data=gateway),
    obj('ServiceAccount','cma-scale-am',automountServiceAccountToken=False),
    obj('ServiceAccount','cma-scale-runtime',automountServiceAccountToken=False),
    obj('Role','cma-scale-am',api='rbac.authorization.k8s.io/v1',rules=[
        {'apiGroups':['agents.x-k8s.io'],'resources':['sandboxes'],'verbs':['get','list','watch','create','patch','delete']},
        {'apiGroups':['extensions.agents.x-k8s.io'],'resources':['sandboxclaims','sandboxwarmpools','sandboxtemplates'],'verbs':['get','list','watch','create','patch','delete']},
        {'apiGroups':[''],'resources':['pods','persistentvolumeclaims'],'verbs':['get','list','watch']},
    ]),obj('RoleBinding','cma-scale-am',api='rbac.authorization.k8s.io/v1',
        roleRef={'apiGroup':'rbac.authorization.k8s.io','kind':'Role','name':'cma-scale-am'},
        subjects=[{'kind':'ServiceAccount','name':'cma-scale-am','namespace':NS}])]
for name,binary,config,port,ready,live,memory in [
    ('cma-agent-manager','cma','am-local',8080,'/ready','/livez','512Mi'),
    ('cma-event-gateway','cmaeg','eg-local',8090,'/readyz','/healthz','384Mi')]:
    env=[]
    mounts=[{'name':'tmp','mountPath':'/tmp'}]
    volumes=[{'name':'tmp','emptyDir':{'sizeLimit':'64Mi'}}]
    spec={'automountServiceAccountToken':False,
          'securityContext':{'runAsNonRoot':True,'runAsUser':10001,'runAsGroup':10001,'fsGroup':10001,'seccompProfile':{'type':'RuntimeDefault'}},
          'terminationGracePeriodSeconds':760 if binary=='cma' else 735,
          'topologySpreadConstraints':[{'maxSkew':1,'topologyKey':'kubernetes.io/hostname','whenUnsatisfiable':'DoNotSchedule','nodeTaintsPolicy':'Honor',
                                       'labelSelector':{'matchLabels':{'app.kubernetes.io/name':name}}}]}
    if binary=='cma':
        spec['serviceAccountName']='cma-scale-am'
        spec['automountServiceAccountToken']=True
        env=[{'name':'POD_IP','valueFrom':{'fieldRef':{'fieldPath':'status.podIP'}}},
             {'name':'CHRONO_AGENT_MANAGER_RELAY_ADDRESS','value':'http://$(POD_IP):8080'}]
        mounts.append({'name':'local-key','mountPath':'/run/cma-local','readOnly':True})
        volumes.append({'name':'local-key','secret':{'secretName':'am-local','items':[{'key':'sandbox-key','path':'sandbox-key'}],'defaultMode':288}})
        lifecycle={'preStop':{'exec':{'command':['/bin/sh','-c','/usr/local/bin/cma --agent-manager-drain; exec /bin/sleep 30']}}}
    else:
        env=[]
        lifecycle={'preStop':{'sleep':{'seconds':30}}}
        mounts.append({'name':'media','mountPath':'/var/cmaeg/media'})
        volumes.append({'name':'media','emptyDir':{'sizeLimit':'32Mi'}})
    container={'name':'api' if binary=='cma' else 'cmaeg','image':image,'imagePullPolicy':'Never',
        'command':['/usr/local/bin/'+binary],'envFrom':[{'configMapRef':{'name':config}},{'secretRef':{'name':config}}],
        'env':env,'ports':[{'name':'http','containerPort':port}],
        'resources':{'requests':{'cpu':'250m','memory':'256Mi'},'limits':{'cpu':'500m','memory':memory}},
        'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},
        'volumeMounts':mounts,'lifecycle':lifecycle,
        'startupProbe':{'httpGet':{'path':live,'port':port},'periodSeconds':3,'failureThreshold':60},
        'readinessProbe':{'httpGet':{'path':ready,'port':port},'periodSeconds':5,'timeoutSeconds':2},
        'livenessProbe':{'httpGet':{'path':live,'port':port},'periodSeconds':10,'timeoutSeconds':2}}
    spec.update(containers=[container],volumes=volumes)
    items.append(obj('Deployment',name,{'replicas':2,'minReadySeconds':30,
        'strategy':{'type':'RollingUpdate','rollingUpdate':{'maxSurge':1,'maxUnavailable':0}},
        'selector':{'matchLabels':{'app.kubernetes.io/name':name}},
        'template':{'metadata':{'labels':{'app.kubernetes.io/name':name},'annotations':{'cma.chronoai.dev/source-sha':sha}},'spec':spec}},api='apps/v1'))
(ROOT/'manifests/apps.json').write_text(json.dumps({'apiVersion':'v1','kind':'List','items':items},indent=2)+'\n')
print('Rendered two-replica control plane; external provider/identity/runtime fixtures remain required')
