#!/usr/bin/env python3
"""timing.py: CPU and wall seconds per seed for the cost rows C1 and C2 (seeds 1-4, one server process, one request per seed).
usage: SCR=/path/to/work/folder python3 -I timing.py SERVER_BIN PORT CELL[,CELL...] [MONTHS] [SEEDS]
Cells as in uprobe.py (T0, U0, UN, UR). CPU time is read from `ps` for the server process before and after the requests, so it is
independent of the machine's load; wall time is reported beside it. The port must be free."""
import sys, os, json, time, subprocess, urllib.request
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import uprobe


def cpu_seconds(pid):
    out = subprocess.run(["ps", "-o", "cputime=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    parts = out.replace(".", ":").split(":")   # [h:]m:s.cc on macOS: m:ss.cc
    nums = [float(x) for x in out.replace(":", " ").split()]
    if len(nums) == 3:
        return nums[0] * 3600 + nums[1] * 60 + nums[2]
    return nums[0] * 60 + nums[1]


def main():
    server, port = sys.argv[1], int(sys.argv[2])
    cells = sys.argv[3].split(",")
    months = int(sys.argv[4]) if len(sys.argv) > 4 else 36
    seeds = int(sys.argv[5]) if len(sys.argv) > 5 else 4
    env = dict(os.environ, RAYON_NUM_THREADS="1", ECONSIM_CACHE_MB="0", ECONSIM_WARMUP_MONTHS="24", PORT=str(port))
    for cell in cells:
        proc = subprocess.Popen([server], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(3)
            cfg = uprobe.cell_cfg(cell)
            c0 = cpu_seconds(proc.pid); t0 = time.time()
            for s in range(1, seeds + 1):
                body = {"config": cfg, "shocks": [], "months": months, "seeds": [s]}
                req = urllib.request.Request(f"http://127.0.0.1:{port}/api/compare", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
                urllib.request.urlopen(req, timeout=3600).read()
            c1 = cpu_seconds(proc.pid); t1 = time.time()
            print(f"{cell}: {seeds} seeds x {months} months, 5000 households, two arms per request: CPU {(c1 - c0) / seeds:.2f} s per seed, wall {(t1 - t0) / seeds:.2f} s per seed")
        finally:
            proc.terminate(); proc.wait(timeout=30)


if __name__ == "__main__":
    main()
