"""A worker for tests/worker_backends.rs that behaves the same on every platform.

Usage: worker_tool.py MODE [ARGUMENT]
  cat           copy standard input to standard output, byte for byte
  fail          write "broken" to standard error and exit with status 3
  flood         write to standard output until stopped
  sleep         print "begun", then sleep for a minute
  backpressure  write ARGUMENT bytes, then read all input and report its size
  orphan        start a descendant that sleeps for a minute, print its process
                ID, and exit without waiting for it
  env           print the environment variable ARGUMENT, or nothing
"""
import os
import subprocess
import sys
import time

mode = sys.argv[1]
out = sys.stdout.buffer
if mode == "cat":
    out.write(sys.stdin.buffer.read())
elif mode == "fail":
    sys.stderr.write("broken")
    sys.exit(3)
elif mode == "flood":
    while True:
        out.write(b"x" * 4096)
        out.flush()
elif mode == "sleep":
    out.write(b"begun")
    out.flush()
    time.sleep(60)
elif mode == "backpressure":
    out.write(b"y" * int(sys.argv[2]))
    out.flush()
    sys.stderr.write(str(len(sys.stdin.buffer.read())))
elif mode == "orphan":
    descendant = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(60)"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    out.write(str(descendant.pid).encode())
elif mode == "env":
    out.write(os.environ.get(sys.argv[2], "").encode())
else:
    sys.exit(f"unknown mode {mode}")
