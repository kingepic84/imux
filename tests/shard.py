#!/usr/bin/env python3
"""Print the tests that belong to one shard of the full suite, one file name per line.

    tests/shard.py SHARD COUNT

The tests are spread so every shard takes about the same time: longest first, each to the shard
with the least time so far. The times come from tests/shard-times.tsv (name, seconds) as measured
in CI; a test that is not in the file counts as the median, so a new test still lands somewhere
sensible. Update the file now and then with the times a run prints. Every shard computes the same
split, so no coordination is needed.
"""
import os
import statistics
import sys

here = os.path.dirname(os.path.abspath(__file__))
shard, count = int(sys.argv[1]), int(sys.argv[2])

times = {}
with open(os.path.join(here, "shard-times.tsv")) as f:
    for line in f:
        name, secs = line.split()
        times[name] = float(secs)
median = statistics.median(times.values()) if times else 10.0

names = sorted(n[:-3] for n in os.listdir(here) if n.endswith(".sh") and n != "run_all.sh")
names.sort(key=lambda n: (-times.get(n, median), n))
load = [0.0] * count
mine = []
for n in names:
    k = min(range(count), key=lambda i: (load[i], i))
    load[k] += times.get(n, median)
    if k == shard:
        mine.append(n + ".sh")
print("\n".join(sorted(mine)))
