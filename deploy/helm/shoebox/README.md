# shoebox Helm chart

Single-replica `shoebox-server` for Kubernetes ≥ 1.25.

## Quick install

```bash
helm install shoebox ./shoebox \
  --set image.repository=ghcr.io/<owner>/shoebox-server \
  --set storage.photos.existingClaim=my-photos-pvc
```

The chart auto-generates `SHOEBOX_SECRET` on first install. Retrieve it
afterwards:

```bash
kubectl get secret shoebox-bootstrap \
  -o jsonpath='{.data.SHOEBOX_SECRET}' | base64 -d
```

Share that string with each desktop client during enrollment.

## Values

See `values.yaml` for the full surface. Constraints:

- Either `secret.create=true` **or** `secret.existingSecret` non-empty.
- Either `storage.photos.existingClaim` **or** `storage.photos.hostPath`.

Chart install fails fast with a clear message if either constraint is
violated.

## Uninstall

```bash
helm uninstall shoebox
# The bootstrap Secret is kept (resource-policy: keep) so existing
# client certs continue to authenticate after reinstall.
# Drop it manually if you really want a fresh start:
kubectl delete secret shoebox-bootstrap
```

## What's NOT in the chart

By design (backlog, see spec §12):
- Ingress (mTLS terminates at the server; expose via Service + LB/VPN).
- NetworkPolicy.
- ServiceMonitor (use a manual PodMonitor or scrape config against :9001).
- HPA (single replica only in v0.x).
