# Fluzo local observability stack (OBS-02)

Docker Compose stack for local diagnostics: OTel Collector, Tempo, Prometheus,
Loki and Grafana. Everything binds to loopback only. Application export stays an
explicit opt-in (OBS-01 `telemetry.otlp.*` settings); running this stack does
not enable export by itself.

Design baseline: PRD 44.7 / Architecture A09
(`fluzo-docs@60c5b0732fb710cdf705476cee8d9156a5ecd971`).

## Requirements

- Docker with the Compose plugin, x86_64 Linux.
- Python 3.11+ for the smoke driver.
- The five pinned images below already pulled (no `:latest` anywhere):

| Image | Digest |
| --- | --- |
| `otel/opentelemetry-collector-contrib` | `sha256:faf125d656fa47cea568b2f3b4494efd2525083bc75c1e96038bc23f05cd68fd` (v0.139.0) |
| `grafana/tempo` | `sha256:65a5789759435f1ef696f1953258b9bbdb18eb571d5ce711ff812d2e128288a4` (v2.9.0) |
| `prom/prometheus` | `sha256:63805ebb8d2b3920190daf1cb14a60871b16fd38bed42b857a3182bc621f4996` (v3.5.0) |
| `grafana/loki` | `sha256:4c431d2e6b9b38718694b31c5d56be7c80dc69c513215fde1aeb5b02cd4e2665` (v3.5.0) |
| `grafana/grafana` | `sha256:70d9599b186ce287be0d2c5ba9a78acb2e86c1a68c9c41449454d0fc3eeb84e8` (v12.3.0) |

## Quick start

```sh
cd observability
cp .env.example .env        # or let the smoke script generate strong values
python3 ../scripts/observability_smoke.py all
```

`all` runs: `up` -> `verify` -> `outage` -> `restart`. Individual steps:

- `up`: generate `.env` if missing (never overwrites), start the stack, wait
  for the collector OTLP HTTP endpoint from the host (the collector image is
  distroless and has no shell for an in-container healthcheck).
- `verify`: publish one synthetic trace+log+metric run through the real
  OTLP export sink (`cargo test -p fluzo-runtime --test otlp_compose
  -- --ignored`) and confirm it in Tempo, Loki and Prometheus with the same
  trace id.
- `outage`: stop the collector, publish 500 records, assert the producer
  never blocks (bounded queue, drops counted by reason), restart the
  collector and confirm new signals arrive.
- `restart`: publish a run, restart Tempo/Loki/Prometheus, confirm the run
  is still queryable (named volumes).
- `down`: stop containers, keep volumes.
- `teardown`: stop and destroy volumes.

## Ports (loopback only)

| Port | Service |
| --- | --- |
| `127.0.0.1:3000` | Grafana UI (`GRAFANA_ADMIN_USER` / `GRAFANA_ADMIN_PASSWORD`) |
| `127.0.0.1:4317` | OTLP gRPC into the collector |
| `127.0.0.1:4318` | OTLP HTTP into the collector (`/v1/traces`, `/v1/logs`, `/v1/metrics`) |

Tempo (3200), Prometheus (9090), Loki (3100) and the collector health port
(13133) are **not** published to the host; query them from inside the
compose network (the smoke script uses `docker exec ... wget`).

## Pointing Fluzo at the stack

Export stays opt-in. With the stack running, configure the OBS-01 sink
endpoint to `http://127.0.0.1:4318` (http/protobuf). Nothing else in the
runtime changes; the collector is just another `DiagnosticSink` destination.

## Credentials and rotation

- Grafana admin credentials come from `observability/.env` (git-ignored).
  The smoke script generates a random password on first `up` and prints it.
- Rotate by editing `.env` and running
  `docker compose --env-file .env up -d --force-recreate grafana`.
- No credentials are baked into images, configs or committed files. The
  collector->backend lanes are unauthenticated loopback-only traffic.

## Data retention

- Tempo blocks: 72 h (`compactor.compaction.block_retention`).
- Loki: 72 h retention.
- Prometheus: 7 d TSDB retention.
- All data lives in named volumes (`fluzo-observability_*`); `teardown`
  destroys them. Nothing here is an audit record (SQLite audit is separate).

## Known image quirks (verified during bring-up)

- `otel/opentelemetry-collector-contrib` is distroless: no shell, so no
  in-container healthcheck; readiness is proven from the host.
- `grafana/tempo` 2.9 has busybox at `/busybox` and **no `/bin/sh`**, so
  its healthcheck uses `CMD` with `/busybox/wget`, not `CMD-SHELL`.
- Tempo 2.9 trace-by-ID HTTP route is `/api/traces/<id>` (plural);
  `/api/trace/<id>` returns 404.
- Tempo `/ready` can report 503 for up to ~60 s while rings settle; the
  healthcheck uses `start_period: 90s`.
- Tempo 2.9 config schema differs from the "latest" (3.x) docs; validate
  changes with `docker run --rm -v .../tempo.yaml:/c.yaml:ro --entrypoint
  sh <tempo> -c '/tempo -config.file=/c.yaml -verify-config'`.
- Prometheus needs `--web.enable-remote-write-receiver` only if the Tempo
  metrics generator is enabled; it is currently disabled (see tempo.yaml).

## Limits

- This is a development/diagnostics stack, not a required CI gate. The
  Docker-capable diagnostics job runs it; normal CI stays Docker-free.
- No HA, no TLS, no authentication between collector and backends.
- Grafana dashboards are provisioned from
  `grafana/dashboards/fluzo-overview.json`; visual review is a human gate.
