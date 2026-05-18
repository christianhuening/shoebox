# Helm chart CI fixtures

Golden files for the helm-lint workflow's `helm template` diff check.
SHOEBOX_SECRET is redacted to `<REDACTED>` because `randAlphaNum`
regenerates per render (Task 9's `lookup` only finds an existing
value when running against a live cluster).

## Regenerate after intentional chart changes

```bash
helm template release deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  | sed -E 's|^(  SHOEBOX_SECRET: ).*$|\1"<REDACTED>"|' \
  > deploy/helm/shoebox/ci/golden-defaults.yaml

helm template release deploy/helm/shoebox \
  -f deploy/helm/shoebox/ci/values-cache-on.yaml \
  | sed -E 's|^(  SHOEBOX_SECRET: ).*$|\1"<REDACTED>"|' \
  > deploy/helm/shoebox/ci/golden-cache-on.yaml
```
