#!/usr/bin/env python3
"""lcb.py FILE.jsonl — run control requests from a JSON-lines file ({"method":..., "params":...} or {"sleep": s})."""
import json, socket, sys, time
def call(method, params):
    s = socket.create_connection(("127.0.0.1", 7980), timeout=180)
    s.sendall((json.dumps({"id": 1, "method": method, "params": params}) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"):
        c = s.recv(1 << 20)
        if not c: break
        buf += c
    return json.loads(buf)
for line in open(sys.argv[1]):
    line = line.strip()
    if not line or line.startswith("#"): continue
    r = json.loads(line)
    if "sleep" in r:
        time.sleep(r["sleep"]); continue
    out = call(r["method"], r.get("params", {}))
    print(r["method"], json.dumps(out)[:160])
