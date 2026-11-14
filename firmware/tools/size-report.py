#!/usr/bin/env python3
"""Flash use of the native image by module and by function (CI log).

    tools/size-report.py ELF [LLVM-NM]

Region A2 (zero-wait, below 0x08008000), B (zero-wait, below 0x08080000:
the hot code and the .data image) and C (.cold, slow flash, from
0x08080000): the totals per module (opendm40::<module>, or the crate for
core / alloc / compiler-builtins), then the largest symbols of B and of C.
Code (t) and read-only data (r: fonts, tables) both count. A guide for
"what to cut or move" (owner 2026-11-13), not a budget check: build.sh
does those.
"""
import collections, os, re, subprocess, sys

elf = sys.argv[1]
nm = sys.argv[2] if len(sys.argv) > 2 else os.environ.get('DM40_LLVMBIN', '') + '/llvm-nm'
out = subprocess.run([nm, '-S', '-C', '--size-sort', elf], capture_output=True, text=True, check=True).stdout


def region(a):
    if a < 0x08008000:
        return 'A2'
    if a < 0x08080000:
        return 'B'
    return 'C'


def module(name):
    n = re.sub(r'^<|^_<', '', name)
    n = re.sub(r'^\w+ as ', '', n)
    m = re.match(r'(opendm40)::(\w+)', n)
    if m:
        return m.group(2)
    m = re.match(r'(\w+)::', n)
    return m.group(1) if m else '(other)'


tot = collections.defaultdict(lambda: collections.Counter())
syms = {'B': [], 'C': [], 'A2': []}
for line in out.splitlines():
    p = line.split(None, 3)
    if len(p) < 4 or p[2].lower() not in 'trd':
        continue
    a, sz, kind, name = int(p[0], 16), int(p[1], 16), p[2].lower(), p[3]
    if a < 0x08000000 or a >= 0x080A0000 or sz == 0:
        continue
    r = region(a)
    tot[module(name)][r] += sz
    syms[r].append((sz, kind, name))

print('Flash use by module (bytes): B (zero-wait) / C (.cold) / A2')
rows = sorted(tot.items(), key=lambda kv: -(kv[1]['B'] + kv[1]['C'] + kv[1]['A2']))
for m, c in rows[:60]:
    print('  %-22s B %7d  C %6d  A2 %5d  total %7d' % (m, c['B'], c['C'], c['A2'], c['B'] + c['C'] + c['A2']))
# App's methods (app*.rs) by name prefix (sp_ scope page, log_ logger,
# pw_ power meter, ...): which feature the 130 KB of `app` is.
grp = collections.defaultdict(lambda: collections.Counter())
for r, lst in syms.items():
    for sz, kind, name in lst:
        m = re.match(r'<opendm40::app::App>::(\w+)', name)
        if m:
            k = m.group(1).split('_')[0] if '_' in m.group(1) else m.group(1)
            grp[k][r] += sz
print("App's methods by prefix (bytes): B / C")
for k, c in sorted(grp.items(), key=lambda kv: -(kv[1]['B'] + kv[1]['C']))[:45]:
    print('  %-16s B %7d  C %6d' % (k, c['B'], c['C']))
for r in ('B', 'C'):
    print('Largest symbols in region %s:' % r)
    for sz, kind, name in sorted(syms[r], reverse=True)[:50]:
        print('  %6d %s %s' % (sz, kind, name[:150]))
