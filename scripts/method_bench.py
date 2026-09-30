#!/usr/bin/env python3
"""Run a repeatable, alternating wrk comparison from a folder profile."""
import argparse
import importlib.metadata
import json
import os
import platform
import re
import signal
import shutil
import statistics
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path


def host_metadata() -> dict:
    try:
        ram_bytes = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        ram_bytes = None
    versions = {}
    for package in ("Django", "granian"):
        try:
            versions[package.lower()] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            versions[package.lower()] = None
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True)
    return {"platform": platform.platform(), "machine": platform.machine(),
            "processor": platform.processor(), "logical_cpus": os.cpu_count(),
            "ram_bytes": ram_bytes, "python": sys.version,
            "django_version": versions["django"], "granian_version": versions["granian"],
            "rustc_version": (rustc.stdout + rustc.stderr).strip() if rustc.returncode == 0 else None}


def run_wrk(url: str, threads: int, connections: int, duration: int, script_path: str | None = None) -> dict:
    script = Path(script_path) if script_path else Path(__file__).with_name("wrk_percentiles.lua")
    command = ["wrk", "-t", str(threads), "-c", str(connections), "-d", f"{duration}s",
               "--latency", "-s", str(script), url]
    result = subprocess.run(command, capture_output=True, text=True)
    output = result.stdout + "\n" + result.stderr
    if result.returncode:
        raise RuntimeError(f"wrk failed ({result.returncode}):\n{output}")
    rps = re.search(r"Requests/sec:\s*([\d.]+)", output)
    if not rps:
        raise RuntimeError(f"Could not parse Requests/sec from wrk output:\n{output}")
    percentiles = {}
    for key, marker in (("p95", "CUSTOM_P95_MS"), ("p99", "CUSTOM_P99_MS")):
        match = re.search(rf"{marker}:\s*([\d.]+)", output)
        if match:
            percentiles[key] = float(match.group(1))
    errors = {"non_2xx_or_3xx": 0, "connect": 0, "read": 0, "write": 0, "timeout": 0}
    match = re.search(r"Non-2xx or 3xx responses:\s*(\d+)", output)
    if match:
        errors["non_2xx_or_3xx"] = int(match.group(1))
    match = re.search(r"Socket errors:\s*connect\s+(\d+),\s*read\s+(\d+),\s*write\s+(\d+),\s*timeout\s+(\d+)", output)
    if match:
        for key, value in zip(("connect", "read", "write", "timeout"), match.groups()):
            errors[key] = int(value)
    total_rps = float(rps.group(1))
    successful_rps = max(0.0, total_rps - errors["non_2xx_or_3xx"] / duration)
    return {"rps": total_rps, "successful_rps": successful_rps,
            "latency_percentiles_ms": percentiles,
            "errors": errors, "command": command, "raw_output": output}


def summarize(samples: list[dict]) -> dict:
    result = {"rounds": len(samples), "mean_rps": statistics.mean(x["rps"] for x in samples),
              "mean_successful_rps": statistics.mean(x["successful_rps"] for x in samples)}
    if len(samples) > 1:
        result["stdev_rps"] = statistics.stdev(x["rps"] for x in samples)
    for key in ("p95", "p99"):
        values = [x["latency_percentiles_ms"][key] for x in samples if key in x["latency_percentiles_ms"]]
        result[f"median_{key}_ms"] = statistics.median(values) if values else None
    result["errors"] = {key: sum(x["errors"][key] for x in samples) for key in samples[0]["errors"]}
    return result


def paired_summary(rounds: list[dict], baseline: str, treatment: str) -> dict:
    by_cycle = {}
    for row in rounds:
        by_cycle.setdefault(row["cycle"], {})[row["variant"]] = row
    pairs = [by_cycle[c] for c in sorted(by_cycle) if baseline in by_cycle[c] and treatment in by_cycle[c]]
    output = {"baseline": baseline, "treatment": treatment, "paired_cycles": len(pairs)}
    for metric in ("rps", "successful_rps"):
        diffs = [pair[treatment][metric] - pair[baseline][metric] for pair in pairs]
        mean = statistics.mean(diffs)
        sd = statistics.stdev(diffs) if len(diffs) > 1 else 0.0
        critical = 2.262 if len(diffs) == 10 else (2.776 if len(diffs) == 5 else 1.96)
        half = critical * sd / (len(diffs) ** 0.5) if diffs else 0.0
        base_mean = statistics.mean(pair[baseline][metric] for pair in pairs)
        output[metric] = {"mean_treatment_minus_baseline": mean, "sample_sd": sd,
                          "ci95_low": mean - half, "ci95_high": mean + half,
                          "relative_to_baseline_pct": 100 * mean / base_mean if base_mean else None}
    for percentile in ("p95", "p99"):
        diffs = [pair[treatment]["latency_percentiles_ms"][percentile]
                 - pair[baseline]["latency_percentiles_ms"][percentile]
                 for pair in pairs
                 if percentile in pair[treatment]["latency_percentiles_ms"]
                 and percentile in pair[baseline]["latency_percentiles_ms"]]
        output[f"{percentile}_treatment_minus_baseline_median_ms"] = statistics.median(diffs) if diffs else None
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, required=True, help="folder's comparison.json")
    parser.add_argument("--cycles", type=int, default=10)
    parser.add_argument("--duration", type=int, default=10, help="seconds per measured round")
    parser.add_argument("--connections", type=int, default=64)
    parser.add_argument("--threads", type=int, default=4, help="wrk generator threads")
    parser.add_argument("--warmup", type=int, default=2, help="warmup rounds per endpoint")
    parser.add_argument("--cooldown", type=float, default=2)
    parser.add_argument("--json-out", type=Path)
    args = parser.parse_args()
    if args.cycles <= 0 or args.duration <= 0 or args.connections <= 0 or args.threads <= 0 or args.warmup < 0:
        raise RuntimeError("cycles, duration, connections and threads must be positive; warmup cannot be negative")
    if shutil.which("wrk") is None:
        raise RuntimeError("wrk is required (install it with your platform's package manager).")
    profile = json.loads(args.profile.read_text(encoding="utf-8"))
    variants = profile["variants"]
    if len(variants) != 2:
        raise RuntimeError("A profile must define exactly two variants: baseline and treatment.")

    processes = []
    for variant in variants:
        command = variant.get("command")
        if command:
            env = os.environ.copy()
            env.update(variant.get("env", {}))
            proc = subprocess.Popen(command, cwd=variant.get("cwd", "."), env=env,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                    start_new_session=True)
            processes.append(proc)

    try:
        if processes:
            for variant in variants:
                for _ in range(120):
                    try:
                        with urllib.request.urlopen(variant["url"], timeout=1) as response:
                            if response.status < 500:
                                break
                    except urllib.error.HTTPError as exc:
                        if exc.code < 500:
                            break
                    except Exception:
                        if any(proc.poll() is not None for proc in processes):
                            raise RuntimeError(f"Server exited before readiness: {variant['label']}")
                        time.sleep(0.25)
                else:
                    raise RuntimeError(f"Server did not become ready: {variant['label']} ({variant['url']})")

        for variant in variants:
            for _ in range(args.warmup):
                warm = run_wrk(variant["url"], args.threads, args.connections, args.duration,
                               profile.get("request_script"))
                if any(warm["errors"].values()) and not profile.get("allow_warmup_errors", False):
                    raise RuntimeError(f"Warmup failed for {variant['label']}: {warm}")

        rounds = []
        samples = {v["label"]: [] for v in variants}
        for cycle in range(args.cycles):
            order = variants[cycle % 2:] + variants[:cycle % 2]
            for position, variant in enumerate(order):
                sample = run_wrk(variant["url"], args.threads, args.connections, args.duration,
                                 profile.get("request_script"))
                record = {"cycle": cycle + 1, "position": position + 1, "variant": variant["label"], **sample}
                rounds.append(record)
                samples[variant["label"]].append(record)
                pct = sample["latency_percentiles_ms"]
                print(f"cycle {cycle + 1}/{args.cycles} {variant['label']}: {sample['rps']:.1f} RPS "
                      f"({sample['successful_rps']:.1f} 2xx/s), "
                      f"p95={pct.get('p95', float('nan'))} ms, p99={pct.get('p99', float('nan'))} ms, "
                      f"errors={sum(sample['errors'].values())}", flush=True)
                if args.cooldown:
                    time.sleep(args.cooldown)

        summaries = {name: summarize(items) for name, items in samples.items()}
        paired = paired_summary(rounds, variants[0]["label"], variants[1]["label"])
        output = args.json_out or args.profile.parent / "results.json"
        output.parent.mkdir(parents=True, exist_ok=True)
        version = subprocess.run(["wrk", "--version"], capture_output=True, text=True)
        data = {
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "host": host_metadata(),
        "profile": profile,
        "load": {"generator": "wrk", "version": (version.stdout + version.stderr).strip().splitlines()[0],
                 "cycles": args.cycles, "duration_seconds": args.duration,
                 "threads": args.threads, "connections": args.connections,
                 "warmup_rounds_per_variant": args.warmup, "cooldown_seconds": args.cooldown},
        "summary": summaries,
        "paired_comparison": paired,
        "rounds": rounds,
        "notes": ["Order alternates between baseline-first and treatment-first by cycle.",
                  "p95/p99 are per-round wrk latency percentiles collected through the included Lua script; summary reports their median across rounds.",
                  "Both endpoints must use the same Django application, method, path and response body."],
        }
        output.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"\nSummary: {json.dumps(summaries, ensure_ascii=False)}")
        print(f"Paired deltas: {json.dumps(paired, ensure_ascii=False)}\nSaved {output}")
        return 0
    finally:
        for proc in processes:
            if proc.poll() is None:
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except (ProcessLookupError, PermissionError):
                    proc.terminate()
        for proc in processes:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    raise SystemExit(main())
