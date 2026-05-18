{{/* Fully qualified release name. */}}
{{- define "shoebox.fullname" -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Chart label. */}}
{{- define "shoebox.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Common labels block. */}}
{{- define "shoebox.labels" -}}
helm.sh/chart: {{ include "shoebox.chart" . }}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{/* Selector labels (must match deployment & service). */}}
{{- define "shoebox.selectorLabels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/* Name of the Secret holding SHOEBOX_SECRET. */}}
{{- define "shoebox.secretName" -}}
{{- if .Values.secret.existingSecret -}}
{{- .Values.secret.existingSecret -}}
{{- else -}}
{{- printf "%s-bootstrap" (include "shoebox.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{/* Validate the secret config. */}}
{{- define "shoebox.validateSecret" -}}
{{- if and .Values.secret.create .Values.secret.existingSecret -}}
{{- fail "secret.create=true and secret.existingSecret are mutually exclusive — set one or the other." -}}
{{- end -}}
{{- if and (not .Values.secret.create) (not .Values.secret.existingSecret) -}}
{{- fail "Either secret.create=true or secret.existingSecret must be set." -}}
{{- end -}}
{{- end -}}

{{/* Validate the photos volume config. */}}
{{- define "shoebox.validatePhotos" -}}
{{- if and .Values.storage.photos.existingClaim .Values.storage.photos.hostPath -}}
{{- fail "storage.photos.existingClaim and storage.photos.hostPath are mutually exclusive — set one or the other." -}}
{{- end -}}
{{- if and (not .Values.storage.photos.existingClaim) (not .Values.storage.photos.hostPath) -}}
{{- fail "Either storage.photos.existingClaim or storage.photos.hostPath must be set so the server can find the photo library." -}}
{{- end -}}
{{- end -}}
