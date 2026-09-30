#!/usr/bin/env python3
"""Measure cold-start time to the first successful Django response."""
import argparse
import json
import os
import signal
import statistics
import subprocess
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit


def readiness_url(url: str) -> str:
    parsed = urlsplit(url)
    return urlunsplit((parsed.scheme, parsed.netloc, "/rps_plain", "", ""))


def wait_until_ready(proc: subprocess.Popen, url: str, timeout: float) -> float:
    started = time.perf_counter()
    deadline = started + timeout
    while time.perf_counter() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"Server exited during startup with status {proc.returncode}")
        try:
            with urllib.request.urlopen(url, timeout=0.5) as response:
                if 200 <= response.status < 300:
                    response.read()
                    return time.perf_counter() - started
        except urllib.error.HTTPError as exc:
            if 200 <= exc.code < 300:
                return time.perf_counter() - started
        except Exception:
            pass
        time.sleep(0.025)
    raise TimeoutError(f"No successful response from {url} within {timeout}s")


def stop_process(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            proc.kill()
        proc.wait()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=40,
                        help="cold starts per variant (recommended range: 30-50)")
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--json-out", type=Path)
    args = parser.parse_args()
    if args.runs < 1 or args.timeout <= 0:
        raise RuntimeError("runs and timeout must be positive")

    profile = json.loads(args.profile.read_text(encoding="utf-8"))
    variants = profile["variants"]
    if len(variants) != 2:
        raise RuntimeError("Startup comparison expects exactly two variants")

    measurements = []
    for cycle in range(args.runs):
        order = variants[cycle % 2:] + variants[:cycle % 2]
        for position, variant in enumerate(order, start=1):
            command = variant.get("command")
            if not command:
                raise RuntimeError(f"Variant has no start command: {variant['label']}")
            env = os.environ.copy()
            env.update(variant.get("env", {}))
            proc = subprocess.Popen(
                command,
                cwd=variant.get("cwd", "."),
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            try:
                seconds = wait_until_ready(proc, readiness_url(variant["url"]), args.timeout)
                record = {"cycle": cycle + 1, "position": position,
                          "variant": variant["label"], "startup_seconds": seconds}
                measurements.append(record)
                print(f"startup {cycle + 1}/{args.runs} {variant['label']}: {seconds * 1000:.2f} ms",
                      flush=True)
            finally:
                stop_process(proc)

    grouped = {variant["label"]: [m["startup_seconds"] for m in measurements
                                   if m["variant"] == variant["label"]]
               for variant in variants}
    summary = {}
    for label, values in grouped.items():
        summary[label] = {
            "runs": len(values),
            "mean_ms": statistics.mean(values) * 1000,
            "median_ms": statistics.median(values) * 1000,
            "stdev_ms": statistics.stdev(values) * 1000 if len(values) > 1 else 0,
            "min_ms": min(values) * 1000,
            "max_ms": max(values) * 1000,
        }

    base_label, treatment_label = [variant["label"] for variant in variants]
    by_cycle = {}
    for row in measurements:
        by_cycle.setdefault(row["cycle"], {})[row["variant"]] = row["startup_seconds"]
    paired_ms = [(pair[treatment_label] - pair[base_label]) * 1000
                 for pair in by_cycle.values()
                 if base_label in pair and treatment_label in pair]
    paired = {"treatment_minus_baseline_median_ms": statistics.median(paired_ms),
              "treatment_minus_baseline_stdev_ms": statistics.stdev(paired_ms) if len(paired_ms) > 1 else 0,
              "paired_cycles": len(paired_ms)}
    startup = {
        "definition": "process launch to first successful HTTP 2xx response from /rps_plain",
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "runs_per_variant": args.runs,
        "order": "alternating baseline-first and treatment-first by cycle",
        "timeout_seconds": args.timeout,
        "summary": summary,
        "paired_comparison": paired,
        "measurements": measurements,
    }

    output = args.json_out or args.profile.parent / "results.json"
    data = json.loads(output.read_text(encoding="utf-8")) if output.exists() else {}
    data["startup_measurement"] = startup
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"Startup summary: {json.dumps(summary, ensure_ascii=False)}")
    print(f"Paired startup: {json.dumps(paired, ensure_ascii=False)}")
    print(f"Updated {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
