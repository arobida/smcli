#!/usr/bin/env python3
"""Isolated macOS fff measurements. Usage: python3 profile-macos.py /absolute/path/to/smcli-service

Creates 10k/50k repetitive 1-KiB fixtures; prints temp result path. No real shell history.
CPU uses calibrated Mach time units; RSS and footprint are distinct macOS measures.
"""
import concurrent.futures
import ctypes
import json
import os
from pathlib import Path
import platform
import socket
import statistics
import sys
import subprocess
import tempfile
import threading
import time

BINARY = Path(sys.argv[1]).resolve()
ROOT = Path(tempfile.mkdtemp(prefix='fff-audit-', dir='/tmp'))
LIB = ctypes.CDLL('/usr/lib/libproc.dylib')
class Timebase(ctypes.Structure):
    _fields_ = [('numer', ctypes.c_uint32), ('denom', ctypes.c_uint32)]
TIMEBASE = Timebase()
assert ctypes.CDLL('/usr/lib/libSystem.B.dylib').mach_timebase_info(ctypes.byref(TIMEBASE)) == 0
CPU_SECONDS_PER_TICK = TIMEBASE.numer / TIMEBASE.denom / 1e9
class Usage(ctypes.Structure):
    _fields_ = [('uuid', ctypes.c_byte * 16)] + [(n, ctypes.c_uint64) for n in (
        'user', 'system', 'pkg', 'interrupt', 'pageins', 'wired', 'resident', 'footprint',
        'start', 'exit', 'child_user', 'child_system', 'child_pkg', 'child_interrupt',
        'child_pageins', 'child_elapsed', 'read', 'written')]
def usage(pid):
    u = Usage()
    assert LIB.proc_pid_rusage(pid, 2, ctypes.byref(u)) == 0
    return {'cpu_s': (u.user + u.system) * CPU_SECONDS_PER_TICK, 'rss_mib': u.resident / 2**20,
            'footprint_mib': u.footprint / 2**20, 'read_bytes': u.read, 'written_bytes': u.written}
def disk(root):
    return {str(p.relative_to(root)): {'bytes': p.stat().st_size, 'allocated': p.stat().st_blocks * 512}
            for p in root.rglob('*') if p.is_file()}
class Service:
    def __init__(self, name, **overrides):
        self.home = ROOT / name
        self.home.mkdir()
        self.runtime = self.home / 'run'
        self.runtime.mkdir(mode=0o700)
        env = {k: v for k, v in os.environ.items() if not k.startswith(('SMCLI_', 'XDG_'))}
        env.update(HOME=str(self.home), HISTFILE='', SMCLI_RUNTIME_DIR=str(self.runtime),
                   SMCLI_CONFIG=str(self.home / 'config.json'), SMCLI_SERVICE_IDLE='600', **overrides)
        self.proc = subprocess.Popen([str(BINARY), '--db', str(self.home / 'h.db')], env=env,
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=open(self.home / 'stderr', 'w'))
        end = time.monotonic() + 15
        while not list(self.runtime.glob('*.sock')):
            assert self.proc.poll() is None, (self.home / 'stderr').read_text()
            assert time.monotonic() < end
            time.sleep(.01)
        self.sock = next(self.runtime.glob('*.sock'))
        self.settle()
        self.base = usage(self.proc.pid)
        self.before = disk(self.home)
        self.samples = []
        self.stop_sample = threading.Event()
        def sample():
            while not self.stop_sample.wait(.02):
                try: self.samples.append(usage(self.proc.pid))
                except AssertionError: break
        self.thread = threading.Thread(target=sample)
        self.thread.start()
    def ask(self, op, directory=None, query=None, **extra):
        msg = dict(v=3, op=op, **extra)
        if directory is not None: msg['directory'] = str(directory)
        if query is not None: msg['query'] = query
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(60)
            s.connect(str(self.sock))
            start = time.perf_counter()
            s.sendall((json.dumps(msg) + '\n').encode())
            chunks = []
            while True:
                b = s.recv(65536)
                assert b, 'closed before reply'
                chunks.append(b)
                if b.endswith(b'\n'): break
            answer = json.loads(b''.join(chunks))
            assert answer['ok'], answer
            return answer, (time.perf_counter() - start) * 1000
    def settle(self):
        prev = usage(self.proc.pid)['cpu_s']
        for _ in range(60):
            time.sleep(.5)
            cur = usage(self.proc.pid)['cpu_s']
            if cur - prev < .01: return
            prev = cur
        raise AssertionError('service did not settle in 30 seconds')
    def batch(self, op, directory, query, n=20, **kw):
        before = usage(self.proc.pid)
        start = time.perf_counter()
        answers = [self.ask(op, directory, query, **kw) for _ in range(n)]
        wall = time.perf_counter() - start
        after = usage(self.proc.pid)
        ms = sorted(x[1] for x in answers)
        a = answers[-1][0]
        return {'median_ms': statistics.median(ms), 'p95_ms': ms[int(.95 * (len(ms)-1))],
                'cpu_s': after['cpu_s'] - before['cpu_s'], 'wall_s': wall,
                'hits': len(a.get('files', [])), 'partial': a.get('partial', False),
                'rss_mib': after['rss_mib'], 'footprint_mib': after['footprint_mib']}
    def close(self):
        self.stop_sample.set(); self.thread.join()
        result = {'baseline': self.base, 'end': usage(self.proc.pid),
                  'peak_sampled_rss_mib': max([self.base['rss_mib']] + [u['rss_mib'] for u in self.samples]),
                  'peak_sampled_footprint_mib': max([self.base['footprint_mib']] + [u['footprint_mib'] for u in self.samples]),
                  'storage_before': self.before, 'storage_after': disk(self.home)}
        self.ask('shutdown'); self.proc.wait(timeout=10)
        return result
def fixture(name, n):
    root = ROOT / name
    root.mkdir()
    subprocess.run(['git', 'init', '-q', str(root)], check=True)
    content = ('// commonneedle\n// ab bc cd de ef fg gh hi ij jk kl lm mn no op pq qr rs st tu uv vw wx xy yz\n'
               'pub fn example() { let value = 123; }\n')
    data = (content * 9)[:1024].ljust(1024, ' ').encode()
    for i in range(n):
        d = root / f'src{i // 500:03}'
        d.mkdir(exist_ok=True)
        (d / f'module{i:06}.rs').write_bytes(data if i != n-1 else b'// rareuniqueneedle\n' + data)
    return root
def measure(name, root):
    s = Service(name)
    start = usage(s.proc.pid)
    a, cold = s.ask('files', root, 'module', limit=100, scope=True)
    assert a['files']
    s.settle()
    settled = usage(s.proc.pid)
    out = {'root': str(root), 'cold_files_ms': cold,
           'index_cpu_s': settled['cpu_s'] - start['cpu_s'], 'settled': settled}
    out['files'] = s.batch('files', root, 'module', limit=100, scope=True)
    out['blank'] = s.batch('files', root, '', limit=100, scope=True)
    for q in ['rareuniqueneedle', 'absentuniqueneedle', 'commonneedle', 'abcdefghijklmnopqrstuvxyz']:
        out['grep_' + q] = s.batch('grep', root, q, n=10, limit=100, scope=True)
        if q == 'rareuniqueneedle':
            assert out['grep_' + q]['hits'] == 1, 'rare fixture marker must remain searchable'
    s.settle()
    u = usage(s.proc.pid); time.sleep(3); v = usage(s.proc.pid)
    out['idle_cpu_s_per_3s'] = v['cpu_s'] - u['cpu_s']
    def one(_): return s.ask('grep', root, 'abcdefghijklmnopqrstuvxyz', limit=100, scope=True)[1]
    u = usage(s.proc.pid); start = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool: times = list(pool.map(one, range(8)))
    v = usage(s.proc.pid)
    out['concurrent_8_grep'] = {'wall_s': time.perf_counter()-start, 'cpu_s': v['cpu_s']-u['cpu_s'],
                               'max_latency_ms': max(times)}
    out['service'] = s.close()
    return out
if __name__ == '__main__':
    out = {'machine': {'platform': platform.platform(), 'logical_cpus': os.cpu_count()},
           'binary': str(BINARY), 'root': str(ROOT), 'measurements': {}}
    for n in (10000, 50000):
        print('creating', n, flush=True)
        root = fixture('tree'+str(n), n)
        out['measurements'][str(n)] = measure('service'+str(n), root)
        (ROOT / 'results.json').write_text(json.dumps(out, indent=2))
        print('done', n, json.dumps(out['measurements'][str(n)]), flush=True)
    print('RESULTS', ROOT / 'results.json', flush=True)
