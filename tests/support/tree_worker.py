"""Real detached, double-forked descendants for guardian lifecycle tests."""
import os
from pathlib import Path
import signal
import sys
import time

mode, directory = sys.argv[1:3]
directory = Path(directory)

def mark(name):
    (directory / name).write_text(str(os.getpid()))

mark("root")
if os.fork() == 0:
    os.setsid()
    mark("middle")
    if os.fork() != 0:
        os._exit(0)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    mark("leaf")
    # Do not make host EOF detection depend on inherited data streams.
    os.close(0)
    os.close(1)
    os.close(2)
    (directory / "leaf-ready").touch()
    time.sleep(20)  # Test failure cannot leave a live fixture forever.
    os._exit(0)

until = time.monotonic() + 3
while not (directory / "leaf-ready").exists():
    if time.monotonic() > until:
        raise RuntimeError("descendant startup timed out")
    time.sleep(0.005)
(directory / "ready").touch()
if mode in ("typed", "typed-wait"):
    sys.stdin.buffer.read()
    sys.stdout.buffer.write(bytes.fromhex(sys.argv[3]))
    sys.stdout.buffer.flush()
if mode in ("wait", "typed-wait"):
    time.sleep(20)
elif mode == "signal":
    os.kill(os.getpid(), signal.SIGUSR1)
elif mode == "failure":
    sys.exit(7)
elif mode == "echo":
    sys.stdout.buffer.write(sys.stdin.buffer.read())
    sys.stderr.buffer.write(os.environ.get("MARK", "").encode())
