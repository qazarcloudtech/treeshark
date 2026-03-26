# 🦈 treeshark

**Hunt down the biggest files devouring your disk — blazingly fast.**

A parallel Rust CLI that scans directory trees using all CPU cores, stores results in SQLite with full history and status tracking, and lets you interactively nuke the files you don't need.

```
193,222 files in 0.07s — 2,800,000 files/sec — 12 cores saturated
```

---

## Quick Start

```bash
make setup                              # build + install
treeshark scan --min-size 1GB --path ~  # scan home dir (all cores)
treeshark list                          # view biggest files
treeshark delete                        # interactively nuke files
treeshark stats                         # see DB summary
```

---

## Installation

```bash
make setup      # deps + build + install + default config
# or manually:
cargo build --release
cp target/release/treeshark ~/.cargo/bin/
```

Other make targets:

```bash
make build      # build release binary
make install    # copy to ~/.cargo/bin
make clean      # remove build artifacts + DB
make lint       # clippy
make fmt        # rustfmt
make help       # show all targets
```

---

## Commands

### `treeshark scan` — find big files

Parallel scan using all CPU cores. Results are stored in SQLite.

```bash
treeshark scan                           # use config.yml defaults
treeshark scan --min-size 500MB          # override threshold
treeshark scan --path /home --path /var  # scan specific paths
treeshark scan --threads 4              # limit thread count
treeshark scan --resume                 # continue interrupted scan
```

Ctrl+C during a scan saves progress. Resume with `--resume`.

### `treeshark list` — query results

Reads from SQLite — instant, no re-scan needed. **Scoped to your config's `scan_paths` by default.**

```bash
treeshark list                    # biggest existing files under scan_paths
treeshark list -n 10              # top 10 only
treeshark list --status deleted   # files you already deleted
treeshark list --status missing   # files that vanished between scans
treeshark list --status all       # everything regardless of status
treeshark list -P /usr            # scope to /usr
treeshark list -P ~/Downloads -P ~/Videos  # scope to multiple paths
treeshark list --all              # ignore path scope, show everything in DB
```

### `treeshark delete` — remove files

Interactive multi-select. **Scoped to your config's `scan_paths` by default** — you only see files you actually scanned.

```bash
treeshark delete                  # pick from files under scan_paths
treeshark delete -P ~/Downloads   # only offer files under ~/Downloads
treeshark delete --all            # offer everything in DB
```

Space to toggle, Enter to confirm, then a y/N safety prompt.
Deleted files are marked `deleted` in the DB (not removed from it).

### `treeshark stats` — database summary

```
  Files tracked:       33   (all statuses)
    ├─ exists:       20   2.6 GB
    ├─ deleted:        3   450 MB freed
    └─ missing:       10
```

### `treeshark history` — past scans

```
    ID                  DATE      SCANNED        FOUND       TIME        SPEED   STATUS
    #3   2026-03-26T10:15:02      193,222           20       0.1s  2,800,000/s   completed
    #2   2026-03-26T09:53:06       15,430            6       0.0s    520,000/s   completed
    #1   2026-03-26T09:50:12      193,222           20       0.4s    470,000/s   interrupted
```

### `treeshark reset` — wipe database

```bash
treeshark reset    # interactive confirm, then deletes all data
# or just:
rm treeshark.db    # next scan creates a fresh DB
```

### `treeshark config` — show current settings

```
  min_size:     1GB
  scan_paths:   ["."]
    resolved:   ["/home/user/project"]
  top_n:        50
  exclude:      [".git", "node_modules", ".cache", "target"]
  threads:      0 (0 = all 12 cores)
  database:     /home/user/project/treeshark.db
```

---

## Configuration

`config.yml`:

```yaml
# treeshark config 🦈
min_size: "1GB"          # threshold (B, KB, MB, GB, TB)
scan_paths:              # directories to scan
  - "."
top_n: 50               # how many files to keep/display
exclude:                 # directory names to skip
  - ".git"
  - "node_modules"
  - ".cache"
  - "target"
threads: 0               # 0 = all CPU cores
```

### Size formats

`"500MB"` · `"1GB"` · `"2.5GB"` · `"10GB"` · `"1TB"`

---

## Path Scoping

`list` and `delete` are **scoped to your config's `scan_paths`** by default. If you scan `~/stock`, you only see `~/stock` files — not leftover results from previous scans of `/usr`.

| Flag | Behavior |
|---|---|
| *(default)* | Show files under config's `scan_paths` |
| `-P ~/Downloads` | Show files under `~/Downloads` |
| `-P /a -P /b` | Show files under `/a` or `/b` |
| `--all` | Show everything in the DB |

This also means `mark_missing` only affects files under the paths that were actually scanned — scanning `~/stock` won't mark `/usr` files as missing.

---

## Resume

```bash
treeshark scan --path /home --path /var --path /opt
# Ctrl+C after /home finishes but during /var
#   ⏸ Scan interrupted — progress saved.

treeshark scan --resume
#   ⏩ Resuming scan #3 — 2 of 3 paths remaining
# picks up /var and /opt, skips /home
```

How it works:
- Each scan records its paths and which are completed
- `--resume` loads the last interrupted scan, skips finished paths
- Files already in the DB just get `times_seen` incremented (upsert)

---

## SQLite Schema

All results persist in `treeshark.db`:

### `files` — unique by full path

| Column | Type | Description |
|---|---|---|
| `path` | TEXT PK | Absolute file path |
| `size` | INTEGER | Size in bytes |
| `status` | TEXT | `exists` / `deleted` / `missing` |
| `first_seen_at` | TEXT | First discovery timestamp |
| `last_seen_at` | TEXT | Most recent scan timestamp |
| `deleted_at` | TEXT | When you deleted it (nullable) |
| `times_seen` | INTEGER | Number of scans that found it |

### `scans` — scan history

| Column | Type | Description |
|---|---|---|
| `id` | INTEGER PK | Scan ID |
| `started_at` | TEXT | Start time |
| `finished_at` | TEXT | End time |
| `scan_paths` | TEXT | JSON array of paths |
| `completed_paths` | TEXT | JSON array (for resume) |
| `status` | TEXT | `completed` / `interrupted` |
| `total_scanned` | INTEGER | Files walked |
| `files_found` | INTEGER | Files matching threshold |
| `files_per_sec` | INTEGER | Throughput |

### Direct SQL access

```bash
sqlite3 treeshark.db "SELECT path, size, status FROM files ORDER BY size DESC LIMIT 10"
sqlite3 treeshark.db "SELECT SUM(size) FROM files WHERE status = 'deleted'"
sqlite3 treeshark.db "SELECT * FROM scans ORDER BY id DESC LIMIT 5"
```

---

## File Status Lifecycle

```
  scan finds file
       │
       ▼
   ┌────────┐   treeshark delete   ┌─────────┐
   │ exists │ ────────────────────► │ deleted  │
   └────────┘                       └─────────┘
       │
       │ not found in next scan
       │ (of the same paths)
       ▼
   ┌─────────┐   scan finds it    ┌────────┐
   │ missing │ ──────again───────► │ exists │
   └─────────┘                     └────────┘
```

---

## Why It's Fast

| Technique | Impact |
|---|---|
| **Recursive `rayon::par_iter`** | Every directory's entries are processed across ALL cores — stat() calls truly parallel |
| **Work-stealing scheduler** | Cores never idle — when one finishes a dir, it steals work from others |
| **Atomic counters** | `AtomicU64` with `Relaxed` ordering — zero synchronization overhead |
| **parking_lot Mutex** | Faster than std for the results vec (~100ns hold time) |
| **Early subtree pruning** | Excluded dirs skipped before recursion |
| **No canonicalize()** | Avoids extra syscall per file |
| **Batch DB writes** | Flush per-path in one SQLite transaction |
| **SQLite WAL + 64MB cache** | Fast writes, concurrent reads |
| **LTO + strip + abort** | 3.3MB binary, maximum codegen optimization |

### Scaling

```
threads=1   0.41s   470k files/sec    1.0x
threads=2   0.21s   940k files/sec    2.0x
threads=4   0.11s   1.8M files/sec    3.7x
threads=6   0.09s   2.3M files/sec    4.6x
threads=8   0.08s   2.5M files/sec    5.1x
threads=12  0.07s   2.8M files/sec    5.9x
```

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  walk_parallel(dir)                                          │
│                                                              │
│  std::fs::read_dir(dir) → collect Vec<DirEntry>              │
│                  │                                           │
│                  ▼                                           │
│  entries.par_iter().for_each(|entry| {    ← rayon fans out   │
│      if dir  → walk_parallel(subdir)     ← recursive work   │
│      if file → stat() → size check       ← on ANY core      │
│                  → results.lock().push()                     │
│  })                                                          │
│                                                              │
│  Thread 1: stat stat stat   ← all cores doing real stat()    │
│  Thread 2: stat stat stat      calls in parallel             │
│  Thread N: stat stat stat                                    │
└──────────────────┬───────────────────────────────────────────┘
                   │ per-path flush
                   ▼
          ┌─────────────────┐
          │  treeshark.db   │
          │  files (path PK)│──► list (scoped)
          │  scans (history)│──► history / resume
          └────────┬────────┘
                   │
                   ▼
             ┌───────────┐
             │  delete    │ removes file + marks status='deleted'
             └───────────┘
```

---

## Project Structure

```
treeshark/
├── Makefile          # build, install, clean, lint, run targets
├── config.yml        # YAML config
├── Cargo.toml        # deps + optimized release profile
├── src/
│   ├── main.rs       # CLI (clap) — subcommands, path scoping
│   ├── config.rs     # YAML loader, size parser, thread detection
│   ├── db.rs         # SQLite schema, upsert, scoped queries, stats
│   ├── scanner.rs    # Recursive parallel walker (rayon) + DB writer
│   ├── display.rs    # Pretty tables for files, history, stats
│   └── deleter.rs    # Interactive multi-select + delete + mark in DB
```

---

## License

MIT
