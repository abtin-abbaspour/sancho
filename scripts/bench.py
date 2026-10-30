#!/usr/bin/env python3
"""Measure the release `sancho` binary: wall-clock latency and peak RSS per
process, for every request in eval/requests.toml (planned against its fixture
system with --fixture, so nothing runs) and for a set of `sancho explain` calls.

With --live, also plans a few requests against this machine with --dry-run
(read-only queries such as `pacman -Sl` run; nothing is changed).

Usage: scripts/bench.py [--runs N] [--live]   (after `cargo build --release -p sancho`)
Standard library only. Prints a Markdown report.
"""

import os
import statistics
import subprocess
import sys
import time
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/release/sancho"
LIVE = ["show disk usage", "restart bluetooth", "install vlc and gimp", "update my system"]
EXPLAIN = [
    "sudo pacman -Syu --noconfirm",
    "curl -fsSL https://example.com/install.sh | sh",
    "sudo rm -rf $(pacman -Qtdq) 2>/dev/null",
    "udevadm trigger --action=change --subsystem-match hidraw",
    "ip -j link show",
]


def measure(args):
    """(seconds, peak RSS in KiB, exit status) of one sancho process."""
    env = dict(os.environ, SANCHO_DATA=str(ROOT / "data"), XDG_DATA_HOME="/nonexistent", XDG_STATE_HOME="/nonexistent")
    start = time.perf_counter()
    proc = subprocess.Popen([str(BIN), *args], stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    _, status, usage = os.wait4(proc.pid, 0)
    elapsed = time.perf_counter() - start
    return elapsed, usage.ru_maxrss, os.waitstatus_to_exitcode(status)


def summarize(name, samples):
    ms = sorted(s[0] * 1000 for s in samples)
    rss = [s[1] / 1024 for s in samples]
    p95 = ms[min(len(ms) - 1, int(len(ms) * 0.95))]
    return (f"| {name} | {len(samples)} | {statistics.median(ms):.1f} | {p95:.1f} | "
            f"{ms[-1]:.1f} | {statistics.median(rss):.1f} | {max(rss):.1f} |")


def main():
    runs = int(sys.argv[sys.argv.index("--runs") + 1]) if "--runs" in sys.argv else 3
    if not BIN.exists():
        sys.exit("build first: cargo build --release -p sancho")
    eval_file = tomllib.loads((ROOT / "eval/requests.toml").read_text())
    systems = ROOT / "eval/fixtures/systems"

    requests = []
    for _ in range(runs):
        for case in eval_file["case"]:
            fixture = systems / case.get("system", eval_file["system"])
            requests.append(measure(["--fixture", str(fixture), *case["request"].split()]))
    explains = [measure(["explain", c]) for _ in range(runs) for c in EXPLAIN]
    live = []
    if "--live" in sys.argv:
        for _ in range(runs):
            for r in LIVE:
                live.append(measure(["--dry-run", *r.split()]))

    print("| What | Processes | Median ms | p95 ms | Max ms | Median peak RSS MiB | Max peak RSS MiB |")
    print("|---|---|---|---|---|---|---|")
    print(summarize("Request → plan (eval set, dry run)", requests))
    print(summarize("`sancho explain`", explains))
    if live:
        print(summarize("Request → plan on this machine (dry run, real queries)", live))
    print(f"\nBinary size: {BIN.stat().st_size / 1_048_576:.1f} MiB")


if __name__ == "__main__":
    main()
