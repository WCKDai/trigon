#!/usr/bin/env python3
"""Time the existing public, offline correctness workflow; no CAD app automation."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def default_binary(system):
    filename = "drawing-workflow.exe" if system == "Windows" else "drawing-workflow"
    return Path("target/release/examples") / filename


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path("out/bim-benchmark/workflow-profile"))
    parser.add_argument("--trials", type=int, default=5)
    parser.add_argument("--binary", type=Path, default=default_binary(platform.system()))
    args = parser.parse_args()
    if not 5 <= args.trials <= 100:
        parser.error("use between 5 and 100 measured trials")
    root = Path(__file__).resolve().parents[1]
    binary = (root / args.binary).resolve()
    if not binary.is_file():
        parser.error("build first: cargo build --release --locked -p trigon-engine --example drawing-workflow")
    output = (root / args.out).resolve()
    # Keep prior evidence immutable, including after a failed or interrupted run.
    output.mkdir(parents=True, exist_ok=False)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    tracked_dirty = bool(subprocess.check_output(["git", "diff", "HEAD", "--name-only"], cwd=root))
    report = {
        "schema": "trigon-benchmark/2",
        "metric": "offline_complete_workflow_wall_ms",
        "scope": "Process launch, model, three views in SVG/PNG/PDF/DXF, exact edit/Undo, schedule, native/GLB/IFC export and two IFC readers",
        "not_measured": ["native UI input/GPU presentation", "peak RSS/VRAM", "competitor speed", "human correction time", "cold disk"],
        # Checkout metadata cannot attest to an independently selected executable.
        # Keep the old source/profile keys explicitly unknown rather than inventing
        # an association, including when --binary selects another build or program.
        "revision": None, "tracked_dirty": None, "profile": None,
        "checkout_context": {"revision": revision, "tracked_dirty": tracked_dirty},
        "binary_path": str(binary), "binary_sha256": digest(binary),
        "binary_provenance": {"status": "unverified", "reason": "Executable hash recorded; no verified build-to-source/profile association supplied."},
        "environment": {"system": platform.system(), "release": platform.release(), "machine": platform.machine(), "python": platform.python_version()},
        "warmups": 1, "trials": [], "status": "RUNNING",
    }
    stable_files = ["ground.svg", "street.svg", "section.svg", "sample.glb", "opening-schedule.json"]
    reference = None
    try:
        for trial in range(args.trials + 1):
            directory = output / ("warmup" if trial == 0 else f"trial-{trial:02}")
            start = time.perf_counter()
            result = subprocess.run([str(binary), str(directory)], cwd=root, env={"PATH": os.environ.get("PATH", "")},
                                    capture_output=True, text=True, encoding="utf-8", timeout=180, check=False)
            elapsed = (time.perf_counter() - start) * 1000.0
            (output / f"{directory.name}.log").write_text(result.stdout + result.stderr, encoding="utf-8")
            if result.returncode:
                raise RuntimeError(f"{directory.name} failed with exit {result.returncode}; see its log")
            verification = json.loads((directory / "verification.json").read_text(encoding="utf-8"))
            if verification.get("move_undo_exact") is not True or verification.get("copy_undo_schedule_exact") is not True:
                raise RuntimeError(f"{directory.name} did not prove exact Undo and schedule restoration")
            readers = verification.get("ifc", {}).get("readers", [])
            if len(readers) != 2 or any(reader.get("failures") != 0 for reader in readers):
                raise RuntimeError(f"{directory.name} failed the IFC-reader correctness condition")
            hashes = {name: digest(directory / name) for name in stable_files}
            if reference is None:
                reference = hashes
            if hashes != reference:
                raise RuntimeError(f"{directory.name} changed deterministic geometry/drawing/schedule outputs")
            if trial:
                report["trials"].append({"trial": trial, "wall_ms": elapsed, "correctness": "PASS", "stable_sha256": hashes})
        values = sorted(trial["wall_ms"] for trial in report["trials"])
        median = statistics.median(values)
        report.update(status="PASS", summary={"n": len(values), "median_ms": median,
            "p95_nearest_rank_ms": values[math.ceil(0.95 * len(values)) - 1], "min_ms": values[0], "max_ms": values[-1],
            "mad_ms": statistics.median(abs(value - median) for value in values), "tail_inference": "weak with five trials"})
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        report.update(status="FAIL", error=str(error))
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": report["status"], "summary": report.get("summary"), "results": str(output / "results.json")}))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
