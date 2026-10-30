# Benchmarks

Measured with `scripts/bench.py --runs 5 --live` on the release build
(`cargo build --release -p sancho`). Each row is separate `sancho` processes,
timed from spawn to exit; peak RSS comes from `wait4` for each process.

Machine: AMD Ryzen 9 7950X3D, NVMe, CachyOS (kernel 7.3), 2026-10-30.

| What | Processes | Median ms | p95 ms | Max ms | Median peak RSS MiB | Max peak RSS MiB |
|---|---|---|---|---|---|---|
| Request → plan (eval set, dry run against fixtures) | 820 | 2.2 | 2.6 | 2.9 | 14.4 | 14.6 |
| `sancho explain` | 25 | 2.1 | 2.4 | 3.1 | 14.6 | 14.6 |
| Request → plan on this machine (dry run, real queries) | 20 | 8.8 | 312.5 | 312.5 | 14.6 | 86.8 |

Release binary: 5.0 MiB.

## Reading the numbers

- **sancho itself peaks at about 15 MiB** (the target is < 100 MB). That covers loading
  the whole recipe, primitive and command-note library, matching, resolving and planning.
- **The 86.8 MiB maximum is `pacman`, not sancho.** `wait4` reports the larger of a
  process and the children it waited for. Package requests run `pacman -Sl`, which loads
  every sync database and peaks at about 86 MiB on its own (so do `pacman -Si` and
  `pacman -Ss`). It runs while sancho waits, so a package request's combined peak is
  about 100 MB; every other request stays near 15 MiB.
- **Live latency is the system queries**, not sancho: `pacman -Sl` about 240 ms,
  `pacman -Q` about 50 ms, `udevadm info --export-db` about 220 ms. Service lookups pass
  the user's word to `systemctl list-unit-files` as a pattern, which takes about 5 ms
  instead of about 600 ms for the full list. Each query runs at most once per request.
- Fixture rows show the cost of sancho's own work with queries answered from files.

## Possible improvements

- Read the pacman sync databases directly instead of running `pacman -Sl`, to remove the
  ~86 MiB child (needs zstd/gzip decompression of the `.db` files).
- Query `udevadm` per subsystem (`hidraw`) instead of exporting the whole database.
