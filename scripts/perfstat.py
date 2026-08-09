#!/usr/bin/env python3
"""Parse `sid-perf:` lines into per-phase percentiles.

Usage: perfstat.py LOG [SKIP_LINES]  — ignores the first SKIP_LINES sid-perf
lines (startup frames), then reports p50/p95/max for build/layout/total.
"""
import re
import sys

pat = re.compile(
    r"sid-perf: frame (\S+) build=([\d.]+)ms layout=([\d.]+)ms total=([\d.]+)ms"
)


def pct(xs, p):
    if not xs:
        return float("nan")
    xs = sorted(xs)
    i = min(len(xs) - 1, int(round((len(xs) - 1) * p)))
    return xs[i]


def main():
    path = sys.argv[1]
    skip = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    rows = []
    with open(path, errors="replace") as fh:
        for line in fh:
            m = pat.search(line)
            if m:
                rows.append((float(m.group(2)), float(m.group(3)), float(m.group(4))))
    rows = rows[skip:]
    if not rows:
        print("frames=0")
        return
    names = ("build", "layout", "total")
    print(f"frames={len(rows)}")
    for i, name in enumerate(names):
        xs = [r[i] for r in rows]
        print(
            f"  {name:6s} p50={pct(xs, 0.5):7.3f} p95={pct(xs, 0.95):7.3f} "
            f"max={max(xs):7.3f} mean={sum(xs) / len(xs):7.3f}"
        )


main()
