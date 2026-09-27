"""Independent v1 test worker. Each process consumes one complete request."""
import os
import struct
import sys
import time

mode = sys.argv[1]
request = sys.stdin.buffer.read()
assert request[:7] == b"BWIP\x01\x00\x00"
assert struct.unpack("<Q", request[7:15])[0] == len(request) - 15
count = struct.unpack("<Q", request[15:23])[0]
if mode == "pack":
    body = b"\x05" + request[15:]
else:
    assert count == 1
    body = request[23:]
if mode == "large":
    body = b"\x04" + struct.pack("<Q", 65536) + b"x" * 65536
if mode == "nan":
    body = b"\x02" + struct.pack("<I", 0x7FC00000)
frame = b"BWIP\x01\x00\x01" + struct.pack("<Q", len(body)) + body
if "FRAME" in os.environ:
    frame = bytes.fromhex(os.environ["FRAME"])
if mode == "version":
    frame = frame[:4] + b"\x02" + frame[5:]
if mode == "truncated":
    frame = frame[:-1]
if mode == "extra":
    frame += b"extra"
if mode != "hang":
    sys.stdout.buffer.write(frame)
    sys.stdout.buffer.flush()
if "READY" in os.environ:
    with open(os.environ["READY"], "w", encoding="utf-8") as ready:
        ready.write("ready")
if mode in ("hang", "reply_hang"):
    while True:
        time.sleep(1)
if mode == "exit":
    sys.exit(7)
