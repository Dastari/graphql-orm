#!/usr/bin/env python3
"""Run a CI command with resource snapshots streamed every 60 seconds."""

from pathlib import Path
import resource
import subprocess
import sys
import time


def snapshot():
    """Print host memory, top resident processes, and cgroup memory evidence."""
    print(f"resource-log: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}", flush=True)
    subprocess.run(["free", "-m"], check=False)
    processes = subprocess.run(
        ["ps", "-eo", "pid,ppid,rss,comm", "--sort=-rss"],
        check=False, text=True, stdout=subprocess.PIPE,
    )
    print("\n".join(processes.stdout.splitlines()[:11]), flush=True)
    # GitHub's VM may have no cgroup memory controller; Linux build scopes do.
    try:
        relative = next(
            line[3:] for line in Path("/proc/self/cgroup").read_text().splitlines()
            if line.startswith("0::")
        )
        cgroup = Path("/sys/fs/cgroup") / relative.lstrip("/")
        for name in ("memory.current", "memory.peak", "memory.swap.current", "memory.events"):
            path = cgroup / name
            if path.exists():
                print(f"resource-log: {name} {path.read_text().strip()}", flush=True)
    except (OSError, StopIteration) as error:
        print(f"resource-log: cgroup unavailable: {error}", flush=True)


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: scripts/run-with-resource-logging.py command [args ...]")
    snapshot()
    started = time.monotonic()
    process = subprocess.Popen(sys.argv[1:])
    try:
        while True:
            try:
                status = process.wait(timeout=60)
                break
            except subprocess.TimeoutExpired:
                snapshot()
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait()
        snapshot()
        peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        print(
            f"resource-log: elapsed={time.monotonic() - started:.1f}s "
            f"largest-child-peak-rss={peak} KiB",
            flush=True,
        )
    return status if status >= 0 else 128 - status


if __name__ == "__main__":
    sys.exit(main())
