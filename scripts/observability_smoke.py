#!/usr/bin/env python3
"""OBS-02 maintainer-run local smoke for the observability Compose stack.

NOT a required CI gate (issue #14: Docker-capable job only). Run from
the repository root:

    python3 scripts/observability_smoke.py up        # credentials + stack
    python3 scripts/observability_smoke.py verify    # synthetic run, 3 backends
    python3 scripts/observability_smoke.py outage    # collector outage + recovery
    python3 scripts/observability_smoke.py restart   # restart persistence
    python3 scripts/observability_smoke.py all       # up+verify+outage+restart
    python3 scripts/observability_smoke.py down      # stop, keep volumes
    python3 scripts/observability_smoke.py teardown  # DESTRUCTIVE: remove volumes

Requires Docker + Compose v2 and a locally generated Grafana password.
All output is evidence for the PR; nothing here runs live inference.
"""

import json
import os
import secrets
import subprocess
import sys
import time
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
COMPOSE_FILE = os.path.join(ROOT, "observability", "docker-compose.yml")
ENV_FILE = os.path.join(ROOT, "observability", ".env")


def env(name, default):
    return os.environ.get(name, default)


def grafana_port():
    return int(env("GRAFANA_PORT", "3000"))


def otlp_http_port():
    return int(env("OTLP_HTTP_PORT", "4318"))


def compose(*args, check=True):
    cmd = ["docker", "compose", "-f", COMPOSE_FILE]
    if os.path.exists(ENV_FILE):
        cmd += ["--env-file", ENV_FILE]
    cmd += list(args)
    print("+", " ".join(cmd))
    return subprocess.run(cmd, check=check, text=True, capture_output=True)


def ensure_env():
    if os.path.exists(ENV_FILE):
        print(f"using existing {ENV_FILE}")
        return
    password = secrets.token_hex(16)
    with open(ENV_FILE, "w") as handle:
        handle.write("GRAFANA_ADMIN_USER=admin\n")
        handle.write(f"GRAFANA_ADMIN_PASSWORD={password}\n")
        handle.write(f"GRAFANA_PORT={grafana_port()}\n")
        handle.write(f"OTLP_GRPC_PORT={env('OTLP_GRPC_PORT', '4317')}\n")
        handle.write(f"OTLP_HTTP_PORT={otlp_http_port()}\n")
    os.chmod(ENV_FILE, 0o600)
    print(f"generated local credentials in {ENV_FILE} (gitignored)")
    print(f"Grafana login: admin / {password}")


def http_json(url, timeout=10, data=None, headers=None):
    request = urllib.request.Request(url, data=data, headers=headers or {})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        body = response.read()
        return json.loads(body) if body else {}


def container_get_json(container, url):
    """Backend APIs stay unpublished (loopback-only policy); query them
    from inside the compose network with the image's own wget."""
    result = subprocess.run(
        ["docker", "exec", container, "wget", "-q", "-O-", url],
        check=True,
        capture_output=True,
        timeout=30,
    )
    return json.loads(result.stdout)


def wait_collector(deadline_s=90):
    url = f"http://127.0.0.1:{otlp_http_port()}/v1/traces"
    end = time.time() + deadline_s
    last = None
    while time.time() < end:
        try:
            request = urllib.request.Request(
                url,
                data=b"",
                headers={"Content-Type": "application/x-protobuf"},
            )
            with urllib.request.urlopen(request, timeout=5) as response:
                if response.status == 200:
                    print(f"collector OTLP HTTP ready at {url}")
                    return
            last = "non-200"
        except Exception as error:  # noqa: BLE001 - readiness loop
            last = error
        time.sleep(1)
    raise SystemExit(f"collector did not become ready: {last}")


def run_synthetic(mode="normal"):
    env_vars = dict(os.environ)
    env_vars["FLUZO_COMPOSE_MODE"] = mode
    env_vars["FLUZO_COMPOSE_OTLP_ENDPOINT"] = (
        f"http://127.0.0.1:{otlp_http_port()}"
    )
    result = subprocess.run(
        [
            "cargo", "test", "-p", "fluzo-runtime", "--test", "otlp_compose",
            "--locked", "--offline", "--", "--ignored", "--nocapture",
        ],
        check=True,
        text=True,
        capture_output=True,
        env=env_vars,
        cwd=ROOT,
    )
    out = result.stdout + result.stderr
    values = {}
    for line in out.splitlines():
        if line.startswith("FLUZO_"):
            key, _, value = line.partition("=")
            values[key] = value
            print(line)
    return values


def retry(fn, attempts=40, delay=3, what=""):
    last = None
    for _ in range(attempts):
        try:
            return fn()
        except Exception as error:  # noqa: BLE001 - evidence loop
            last = error
            time.sleep(delay)
    raise RuntimeError(f"{what} not visible after retries: {last}")


def verify_tempo(trace_id):
    data = container_get_json(
        "fluzo-observability-tempo-1",
        f"http://tempo:3200/api/traces/{trace_id}",
    )
    spans = data.get("batches", [])
    assert spans, f"no trace in Tempo for {trace_id}"
    span = spans[0]["scopeSpans"][0]["spans"][0]
    print(f"TEMPO OK: trace={trace_id} span={span['name']} kind={span.get('kind')}")
    return span


def verify_loki(trace_id):
    query = urllib.parse.quote(f'{{service_name="fluzo"}} |~ "{trace_id}"')
    data = container_get_json(
        "fluzo-observability-loki-1",
        "http://loki:3100/loki/api/v1/query_range"
        f"?query={query}&limit=5",
    )
    streams = data["data"]["result"]
    assert streams, f"no Loki stream for trace {trace_id}"
    line = streams[0]["values"][0][1]
    print(f"LOKI OK: {line[:120]}")
    return line


def verify_prometheus():
    query = urllib.parse.quote('count({__name__=~"gen_ai_client.*"})')
    data = container_get_json(
        "fluzo-observability-prometheus-1",
        f"http://prometheus:9090/api/v1/query?query={query}",
    )
    result = data["data"]["result"]
    assert result, "no gen_ai_* metrics scraped from the collector"
    print(f"PROMETHEUS OK: {result[0]['value'][1]} gen_ai series")
    return result


def cmd_up():
    ensure_env()
    compose("up", "-d", "--wait")
    wait_collector()
    print("stack up; Grafana on http://127.0.0.1:%d" % grafana_port())


def cmd_verify():
    values = run_synthetic("normal")
    trace_id = values["FLUZO_SMOKE_TRACE_ID"]
    retry(lambda: verify_tempo(trace_id), what="Tempo trace")
    retry(lambda: verify_loki(trace_id), what="Loki log")
    retry(verify_prometheus, what="Prometheus metrics")
    print("VERIFY OK: all three backends hold the correlated synthetic run")


def cmd_outage():
    print("== collector outage ==")
    compose("stop", "otel-collector")
    values = run_synthetic("outage")
    print(f"producer stayed non-blocking: {values.get('FLUZO_OUTAGE_ELAPSED_MS')} ms")
    print("== collector recovery ==")
    compose("up", "-d", "--wait")
    wait_collector()
    fresh = run_synthetic("normal")
    trace_id = fresh["FLUZO_SMOKE_TRACE_ID"]
    retry(lambda: verify_tempo(trace_id), what="Tempo trace after recovery")
    retry(lambda: verify_loki(trace_id), what="Loki log after recovery")
    print("OUTAGE OK: bounded producer, gaps counted, new signals delivered")


def cmd_restart():
    print("== restart persistence ==")
    values = run_synthetic("normal")
    trace_id = values["FLUZO_SMOKE_TRACE_ID"]
    retry(lambda: verify_tempo(trace_id), what="pre-restart Tempo trace")
    compose("restart", "tempo", "loki", "prometheus")
    # Wait for backends to answer again.
    retry(lambda: verify_tempo(trace_id), attempts=60, what="post-restart Tempo trace")
    retry(lambda: verify_loki(trace_id), attempts=60, what="post-restart Loki log")
    retry(verify_prometheus, attempts=60, what="post-restart Prometheus")
    print("RESTART OK: trace, log and metric data survived backend restart")


def cmd_down():
    compose("down")
    print("stack stopped; named volumes retained")


def cmd_teardown():
    print("DESTRUCTIVE: removing the stack and all backend volumes")
    compose("down", "-v", "--remove-orphans")
    print("volumes removed")


def main():
    commands = {
        "up": cmd_up,
        "verify": cmd_verify,
        "outage": cmd_outage,
        "restart": cmd_restart,
        "down": cmd_down,
        "teardown": cmd_teardown,
    }
    mode = sys.argv[1] if len(sys.argv) > 1 else "all"
    if mode == "all":
        for step in ("up", "verify", "outage", "restart"):
            print(f"\n===== {step} =====")
            commands[step]()
        print("\nALL OK — run 'down' to stop or 'teardown' to destroy volumes")
    elif mode in commands:
        commands[mode]()
    else:
        raise SystemExit(f"unknown mode {mode!r}; see docstring")


if __name__ == "__main__":
    main()
