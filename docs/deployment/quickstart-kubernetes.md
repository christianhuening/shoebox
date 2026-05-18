# Quickstart — Kubernetes (Helm)

For operators running an existing Kubernetes cluster. The chart is
single-replica only in v0.x (HA is backlog).

## Prerequisites

- Kubernetes 1.25+
- Helm 3.13+
- A PersistentVolumeClaim (or hostPath on a single node) pointing at
  your photo library — the chart will not create one.

## Install

Download the chart from the release:
```bash
curl -fsSL -o shoebox-0.1.0.tgz \
  https://github.com/<owner>/shoebox/releases/download/v0.1.0/shoebox-0.1.0.tgz
```

Minimal install with hostPath photos (single-node clusters, k3s, etc.):
```bash
helm install shoebox ./shoebox-0.1.0.tgz \
  --set image.repository=ghcr.io/<owner>/shoebox-server \
  --set storage.photos.hostPath=/srv/photos
```

Realistic install with an existing photos PVC:
```bash
cat > shoebox-values.yaml <<'EOF'
image:
  repository: ghcr.io/<owner>/shoebox-server
  tag: v0.1.0

storage:
  data:
    size: 5Gi
    storageClassName: longhorn
  cache:
    enabled: true
    size: 50Gi
    storageClassName: longhorn
  photos:
    existingClaim: photos-library

service:
  type: ClusterIP

extraEnv:
  - name: SHOEBOX_LOG
    value: info
EOF

helm install shoebox ./shoebox-0.1.0.tgz -f shoebox-values.yaml
```

## Retrieve the bootstrap secret

```bash
kubectl get secret shoebox-shoebox-bootstrap \
  -o jsonpath='{.data.SHOEBOX_SECRET}' | base64 -d ; echo
```

(The secret name is `<release>-shoebox-bootstrap`; with the install
command above the release is `shoebox`, so the secret is
`shoebox-shoebox-bootstrap`.)

Hand that string to each desktop client during first-run enrollment.

## Exposing the service

The chart creates a `ClusterIP` Service called `shoebox-shoebox` with
ports `mtls` (9000) and `health` (9001). mTLS terminates at the server,
so **do not** put a Layer-7 proxy (nginx, traefik, gateway-api HTTPRoute)
in front. Options:

- **`LoadBalancer` Service** — easiest:
  ```bash
  helm upgrade shoebox ./shoebox-0.1.0.tgz \
    -f shoebox-values.yaml \
    --set service.type=LoadBalancer
  ```
- **`NodePort`** — for clusters without a cloud LB.
- **`Ingress` with TCP/SSL passthrough** — works with traefik
  IngressRouteTCP or contour TCPProxy; do NOT use a vanilla L7 Ingress.
- **In-cluster Tailscale subnet router** — clients reach the Service via
  the cluster's tailnet.

## Upgrade

```bash
curl -fsSL -o shoebox-0.2.0.tgz \
  https://github.com/<owner>/shoebox/releases/download/v0.2.0/shoebox-0.2.0.tgz
helm upgrade shoebox ./shoebox-0.2.0.tgz -f shoebox-values.yaml
```

The chart uses `strategy: Recreate` (single replica + RWO data PVC), so
expect a brief downtime during rollout while the old pod releases the
PVC and the new one acquires it.

The bootstrap Secret is preserved across upgrades — the chart uses a
`lookup` guard to reuse the in-cluster value rather than regenerating
`randAlphaNum`, so existing client certs keep authenticating.

## Uninstall

```bash
helm uninstall shoebox
```

The bootstrap Secret (`shoebox-shoebox-bootstrap`) is annotated
`helm.sh/resource-policy: keep` and remains after uninstall — existing
client certs are signed by the CA that's derived from this secret, so
reinstalling reuses it and clients keep working. To wipe everything:

```bash
kubectl delete secret shoebox-shoebox-bootstrap
kubectl delete pvc shoebox-shoebox-data shoebox-shoebox-cache
```

## What the chart does NOT do

By design — see the spec's backlog:

- No `Ingress` (mTLS rules out L7).
- No `NetworkPolicy` (write your own; allow `:9000` from client subnets).
- No `ServiceMonitor` (write a `PodMonitor` against `:9001/metrics` if
  using prometheus-operator).
- No HPA / multi-replica (HA is a sub-project #1 backlog item).
- No automated CA / cert-manager wiring.
