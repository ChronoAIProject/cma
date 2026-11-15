#!/usr/bin/env python3
"""Local equivalents of checked-in GKE HTTPRoute rules; no session affinity."""
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parent.parent
NS='cma-scale'
def obj(kind,name,spec=None,api='v1',**extra):
    o={'apiVersion':api,'kind':kind,'metadata':{'name':name,'namespace':NS},**extra}
    if spec is not None: o['spec']=spec
    return o

nginx='''pid /tmp/nginx.pid;
events { worker_connections 1024; }
http {
  access_log off;
  error_log /dev/stderr warn;
  client_body_temp_path /tmp/client;
  proxy_temp_path /tmp/proxy;
  fastcgi_temp_path /tmp/fastcgi;
  uwsgi_temp_path /tmp/uwsgi;
  scgi_temp_path /tmp/scgi;
  resolver 10.96.0.10 valid=5s ipv6=off;
  ssl_certificate /tls/tls.crt;
  ssl_certificate_key /tls/tls.key;
  ssl_protocols TLSv1.2 TLSv1.3;
  map $http_upgrade $connection_upgrade { default upgrade; '' close; }
  map $http_x_scale_fault $am_backend { default am; delay am_delay; }
  map $http_x_scale_fault $eg_backend { default eg; delay eg_delay; }
  upstream am { zone am 64k; server cma-agent-manager-headless.cma-scale.svc.cluster.local:8080 resolve; }
  upstream eg { zone eg 64k; server cma-event-gateway-headless.cma-scale.svc.cluster.local:8090 resolve; }
  upstream am_delay { server toxiproxy.cma-scale.svc.cluster.local:18081; }
  upstream eg_delay { server toxiproxy.cma-scale.svc.cluster.local:18082; }
  proxy_http_version 1.1;
  proxy_set_header Host $host;
  proxy_set_header X-Forwarded-For $remote_addr;
  proxy_set_header X-Forwarded-Proto https;
  proxy_set_header Upgrade $http_upgrade;
  proxy_set_header Connection $connection_upgrade;
  proxy_buffering off;
  proxy_request_buffering off;
  proxy_read_timeout 660s;
  proxy_send_timeout 660s;
  proxy_connect_timeout 3s;
  proxy_next_upstream off;
  client_max_body_size 32m;
  add_header X-Scale-Upstream $upstream_addr always;
  server {
    listen 8080 ssl default_server;
    server_name _;
    location = /lb-health { return 200 'ok'; }
    location / { return 404; }
  }
  server {
    listen 8080 ssl;
    server_name am.cma-scale.test;
    location / { proxy_pass http://$am_backend; }
  }
  server {
    listen 8080 ssl;
    server_name eg.cma-scale.test;
    location / { proxy_pass http://$eg_backend; }
  }
  server {
    listen 8080 ssl;
    server_name cma-scale.test localhost;
    location ~ ^/event-gateway/(v1|mcp|openapi\\.json|docs)(/|$) {
      rewrite ^/event-gateway/(.*)$ /$1 break;
      proxy_pass http://$eg_backend;
    }
    location ~ ^/event-gateway/(connect|legacy-console)(/|$) { proxy_pass http://$eg_backend; }
    location ~ ^/api/agent-manager(/|$) {
      rewrite ^/api/agent-manager(.*)$ /api$1 break;
      proxy_pass http://$am_backend;
    }
    location ~ ^/(api|auth)(/|$) { proxy_pass http://$am_backend; }
    location = /openapi.json { proxy_pass http://$am_backend; }
    location = /agent-manager/openapi.json { proxy_pass http://$am_backend; }
    location = /.well-known/agent-card.json { proxy_pass http://$am_backend; }
    location / { return 404; }
  }
}
'''
proxies=[{'name':name,'listen':f'0.0.0.0:{port}','upstream':f'{service}:80','enabled':True}
         for name,port,service in [('am',18081,'cma-agent-manager'),('eg',18082,'cma-event-gateway')]]
items=[obj('ConfigMap','routing',data={'nginx.conf':nginx}),obj('ConfigMap','toxiproxy',data={'config.json':json.dumps(proxies)})]
for name,image,command,ports,memory,cpu,volume,mount,path in [
    ('toxiproxy','ghcr.io/shopify/toxiproxy:2.12.0',['/toxiproxy','-host','0.0.0.0','-config','/config/config.json'],[8474,18081,18082],'128Mi','100m','toxiproxy','/config',None),
    ('routing','nginx:1.28-alpine',['nginx','-g','daemon off;','-c','/config/nginx.conf'],[8080],'128Mi','200m','routing','/config','/lb-health')]:
    container={'name':name,'image':image,'command':command,'ports':[{'containerPort':p} for p in ports],
               'resources':{'requests':{'cpu':'25m','memory':'32Mi'},'limits':{'cpu':cpu,'memory':memory}},
               'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},
               'volumeMounts':[{'name':'config','mountPath':mount,'readOnly':True},{'name':'tmp','mountPath':'/tmp'}]}
    if path: container['readinessProbe']={'httpGet':{'path':path,'port':ports[0],'scheme':'HTTPS'},'periodSeconds':5}
    else: container['readinessProbe']={'tcpSocket':{'port':ports[0]},'periodSeconds':5}
    volumes=[{'name':'config','configMap':{'name':volume}},{'name':'tmp','emptyDir':{'sizeLimit':'64Mi'}}]
    if name=='routing':
        container['volumeMounts'].append({'name':'tls','mountPath':'/tls','readOnly':True})
        volumes.append({'name':'tls','secret':{'secretName':'routing-tls','defaultMode':288}})
    items.append(obj('Deployment',name,{'replicas':1,'selector':{'matchLabels':{'app':name}},
        'template':{'metadata':{'labels':{'app':name}},'spec':{'automountServiceAccountToken':False,
        'securityContext':{'runAsNonRoot':True,'runAsUser':10001,'runAsGroup':10001,'fsGroup':10001,'seccompProfile':{'type':'RuntimeDefault'}},
        'containers':[container],'volumes':volumes}}},api='apps/v1'))
    service={'selector':{'app':name},'sessionAffinity':'None','ports':[{'name':f'p{p}','port':p,'targetPort':p} for p in ports]}
    if name=='routing':
        service['type']='NodePort'
        service['ports'][0]['nodePort']=30080
    items.append(obj('Service',name,service))
for name,port in [('cma-agent-manager',8080),('cma-event-gateway',8090)]:
    for headless in [False,True]:
        service={'selector':{'app.kubernetes.io/name':name},'sessionAffinity':'None',
                 'ports':[{'name':'http','port':port if headless else 80,'targetPort':port}]}
        if headless: service['clusterIP']='None'
        items.append(obj('Service',name+('-headless' if headless else ''),service))
(ROOT/'manifests/routing.json').write_text(json.dumps({'apiVersion':'v1','kind':'List','items':items},indent=2)+'\n')
