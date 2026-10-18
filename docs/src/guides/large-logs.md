# Large logs and threads

`logs analyze`, `report` and `logs ingest` read log files on several threads. By default they use every core. `--threads N` sets the number, and `--threads 1` uses one. `RAYON_NUM_THREADS` sets the default.

## Recommended: `--threads 4`

On 40 generated PostgreSQL 17 stderr logs (10.7 GB, 16 logical cores), 4 threads gave most of the speed for a fraction of the memory:

| Threads | Time | Throughput | Peak memory |
|---|---|---|---|
| 1 | 53.8 s | 200 MB/s | 45 MiB |
| 4 | 16.8 s | 640 MB/s | 203 MiB |
| 16 | 8.9 s | 1.2 GB/s | 512 MiB |

Going from 4 to 16 threads took half the time again but used 2.5 times the memory, and a shared machine may not have 16 cores to spare. Version 0.1.1 needed 53 s at 4 threads and 56 s at 16.

```bash
pgloupe logs analyze --threads 4 /var/log/postgresql/*.log
```

The report is the same at every thread count.

## What to expect

- Several files are read at once. One file, or a few files among many threads, has its lines parsed on the idle threads while one thread groups them into entries.
- Memory depends on the number of distinct statements, not on the size of the logs. With 100,000 distinct statements, 16 threads used 0.6 GiB.
- csvlog, jsonlog and pgBouncer logs are read one file per thread, so a single file of those formats stays on one thread.

## Planned for 0.2.0

Version 0.2.0 is planned to default to 4 threads. Nothing changes in 0.1.x: the default is still every core, so pass `--threads 4` to get the recommended setting now.

Figures vary by hardware and log mix.
