#!/usr/bin/env python3
"""Compare two release CLIs on the same local saved drawings, requiring identical SVGs."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import random
import statistics
import subprocess
import time


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--doc", type=Path, required=True)
    parser.add_argument("--views", nargs="+", required=True, help="existing saved view IDs")
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--out", type=Path, default=Path("out/drawing-benchmark"))
    args = parser.parse_args()
    if not 5 <= args.samples <= 100 or not 1 <= args.timeout <= 600:
        parser.error("use 5..100 samples and a 1..600 second timeout")
    if len(args.views) > 256 or len(set(args.views)) != len(args.views):
        parser.error("select at most 256 unique saved views")
    binaries = {"baseline": args.baseline.resolve(strict=True), "candidate": args.candidate.resolve(strict=True)}
    doc = args.doc.resolve(strict=True)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    environment = {
        "platform": platform.platform(),
        "profile_requirement": "Both binaries must use the same release profile and toolchain.",
        "scope": "Fresh CLI process: document load/build, projection, SVG serialization and file write.",
        "noise": "Shared machine; close competing workloads before running. No CPU pinning is applied.",
        "doc": str(doc),
        "document_sha256": sha(doc),
        "binaries": {key: {"path": str(path), "sha256": sha(path)} for key, path in binaries.items()},
        "samples": args.samples,
        "warmups_per_view_and_binary": 1,
        "seed": 27,
    }
    rows = []
    references = {}
    rng = random.Random(27)
    for index, view in enumerate(args.views):
        measured = [(version, sample) for version in binaries for sample in range(args.samples)]
        rng.shuffle(measured)
        order = [(version, -1) for version in binaries] + measured
        for version, sample in order:
            # Use an index, never an untrusted view ID, as the output filename.
            svg = out / f"view-{index}-{version}-{sample}.svg"
            command = [str(binaries[version]), "run", "--doc", str(doc), "view.export", json.dumps({"id": view, "path": str(svg)})]
            start = time.perf_counter()
            result = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout, check=True)
            seconds = time.perf_counter() - start
            digest = sha(svg)
            rows.append({
                "view": view, "version": version, "sample": sample, "seconds": seconds,
                "bytes": svg.stat().st_size, "sha256": digest, "command": command,
                "metrics": json.loads(result.stdout),
            })
            (out / "raw.json").write_text(json.dumps({"environment": environment, "runs": rows}, indent=2) + "\n")
            references.setdefault(view, digest)
            if digest != references[view]:
                raise RuntimeError(f"SVG bytes changed for view {index}; this is not a behavior-preserving performance comparison")
            print(f"view {index} / {version} / {sample}: {seconds:.4f} s", flush=True)
    if sha(doc) != environment["document_sha256"]:
        raise RuntimeError("the source document changed during the comparison")
    for version, path in binaries.items():
        if sha(path) != environment["binaries"][version]["sha256"]:
            raise RuntimeError(f"the {version} executable changed during the comparison")
    summary = {}
    for view in args.views:
        values = {version: [r["seconds"] for r in rows if r["view"] == view and r["version"] == version and r["sample"] >= 0] for version in binaries}
        summary[view] = {}
        for version, times in values.items():
            summary[view][version] = {
                "n": len(times), "median_s": statistics.median(times), "p95_s": percentile(times, 0.95),
                "min_s": min(times), "max_s": max(times), "stdev_s": statistics.stdev(times),
            }
        ratios = [statistics.median(rng.choices(values["candidate"], k=args.samples)) / statistics.median(rng.choices(values["baseline"], k=args.samples)) for _ in range(2000)]
        summary[view]["candidate_over_baseline"] = {
            "median_ratio": statistics.median(values["candidate"]) / statistics.median(values["baseline"]),
            "bootstrap_95_percent_interval": [percentile(ratios, 0.025), percentile(ratios, 0.975)],
        }
    report = {"environment": environment, "svg_bytes_identical": True, "summary": summary}
    (out / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Verified identical SVG bytes. Results: {out / 'summary.json'}")


if __name__ == "__main__":
    main()
