#!/usr/bin/env python3
"""Fake hwglanced for the widget harness.

Serves the fixture snapshot on a Unix socket with moving values, a thermal
throttle flag on the CPU (on) and GPU (off), and answers `history` requests with
a synthetic series. Usage: fake-daemon.py <socket path> [seconds to run]
"""
import json, math, os, socket, sys, threading, time

path = sys.argv[1]
run_for = float(sys.argv[2]) if len(sys.argv) > 2 else 60
base = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), '../../fixtures/snapshot.json')))
base['type'] = 'snapshot'


def flag(rid, label, value, count):
    return {"id": rid, "label": label, "unit": "flag", "value": value, "stats": None,
            "desc": "Whether the part is being slowed down to stay within its temperature limit.",
            "events": {"count": count, "last_unix_ms": int(time.time() * 1000)}}


def readings(snap):
    for g in snap['sensors']:
        for s in g['sections']:
            for r in s['readings']:
                yield r
            if s.get('summary'):
                yield s['summary']


def wobble(rid, t):
    return math.sin(t / 7.0 + (hash(rid) % 17)) * 0.5 + 0.5


def make_snapshot(seq, procs):
    snap = json.loads(json.dumps(base))
    t = time.time()
    snap['seq'] = seq
    snap['unix_ms'] = int(t * 1000)
    for r in readings(snap):
        if r['value'] is None or r['unit'] in ('flag',):
            continue
        r['value'] = r['value'] * (0.85 + 0.3 * wobble(r['id'], t))
    for g in snap['sensors']:
        if g['id'] == 'cpu':
            g['sections'].append({"id": "cpu/throttle", "title": "Throttling", "collapsed_by_default": True,
                                  "readings": [flag("cpu/throttle/thermal", "Thermal", 1.0, 3)]})
        if g['id'] == 'gpu0':
            g['sections'].append({"id": "gpu0/throttle", "title": "Throttling", "collapsed_by_default": True,
                                  "readings": [flag("gpu0/throttle/thermal", "Thermal", 0.0, 0)]})
    if not procs:
        snap['procs'] = None
    return snap


def history(ids, seconds):
    snap = make_snapshot(0, False)
    cur = {r['id']: r for r in readings(snap)}
    now = int(time.time() * 1000)
    out = []
    for rid in ids:
        r = cur.get(rid)
        if not r or r['value'] is None:
            continue
        pts = []
        for k in range(seconds, 0, -1):
            t = now - k * 1000
            pts.append([t, r['value'] * (0.7 + 0.5 * wobble(rid, t / 1000.0))])
        out.append({"id": rid, "unit": r['unit'], "points": pts})
    return {"type": "history", "series": out}


def handle(conn):
    procs = False
    buf = b''
    conn.settimeout(0.2)
    seq = 0
    last = 0
    try:
        while True:
            try:
                data = conn.recv(4096)
                if not data:
                    return
                buf += data
            except socket.timeout:
                pass
            while b'\n' in buf:
                line, buf = buf.split(b'\n', 1)
                msg = json.loads(line)
                print('RECV', json.dumps(msg)[:120], flush=True)
                if msg.get('cmd') == 'subscribe':
                    procs = bool(msg.get('procs'))
                    last = 0
                elif msg.get('cmd') == 'set_procs':
                    procs = bool(msg.get('enabled'))
                elif msg.get('cmd') == 'history':
                    conn.sendall((json.dumps(history(msg['ids'], msg['seconds'])) + '\n').encode())
            if time.time() - last >= 1.0 and last != -1:
                seq += 1
                conn.sendall((json.dumps(make_snapshot(seq, procs)) + '\n').encode())
                last = time.time()
    except (BrokenPipeError, ConnectionResetError):
        pass
    finally:
        conn.close()
        print('CLOSED', flush=True)


if os.path.exists(path):
    os.unlink(path)
srv = socket.socket(socket.AF_UNIX)
srv.bind(path)
srv.listen(4)
srv.settimeout(0.5)
end = time.time() + run_for
while time.time() < end:
    try:
        c, _ = srv.accept()
    except socket.timeout:
        continue
    threading.Thread(target=handle, args=(c,), daemon=True).start()
srv.close()
os.unlink(path)
