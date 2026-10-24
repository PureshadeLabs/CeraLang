#!/usr/bin/env python3
"""Runs a Lythos userspace ELF under Unicorn with a model of the Lythos syscall ABI
(docs/spec/syscalls.md in PureshadeOS). Used by the cera-native tests when the
`unicorn` Python package is available.

usage: lythos_emu.py ELF [ARG...]   -> prints the program's console output, exits with its status
"""
import os, struct, sys
from unicorn import *
from unicorn.x86_const import *

ERR = lambda n: (-n) & 0xFFFFFFFFFFFFFFFF
PAGE = 0x1000
HEAP_BASE = 0x4_0000_0000
STACK_TOP = 0x7FFF_0000
STACK_SIZE = 0x10_0000

def main():
    path, args = sys.argv[1], sys.argv[2:]
    elf = open(path, 'rb').read()
    assert elf[:4] == b'\x7fELF'
    entry, phoff = struct.unpack_from('<QQ', elf, 24)
    phentsize, phnum = struct.unpack_from('<HH', elf, 54)
    uc = Uc(UC_ARCH_X86, UC_MODE_64)
    # Lythos enables SSE/SSE2 at boot; mirror that.
    uc.reg_write(UC_X86_REG_CR0, (uc.reg_read(UC_X86_REG_CR0) & ~0x4) | 0x2)
    uc.reg_write(UC_X86_REG_CR4, uc.reg_read(UC_X86_REG_CR4) | 0x600)
    for i in range(phnum):
        t, flags, off, va, _, filesz, memsz, _ = struct.unpack_from('<IIQQQQQQ', elf, phoff + i * phentsize)
        if t != 1:
            continue
        # The Lythos loader rejects segments below 1 GiB.
        assert va >= 0x4000_0000, f'segment at {va:#x} is below 1 GiB'
        lo, hi = va & ~(PAGE - 1), (va + memsz + PAGE - 1) & ~(PAGE - 1)
        uc.mem_map(lo, hi - lo)
        uc.mem_write(va, elf[off:off + filesz])
    uc.mem_map(STACK_TOP - STACK_SIZE, STACK_SIZE)
    # Initial stack: argc, argv, NULL, envp NULL, auxv (AT_PAGESZ, AT_NULL), strings.
    strings, ptrs = b'', []
    base = STACK_TOP - 0x1000
    for a in [path] + args:
        ptrs.append(base + len(strings))
        strings += a.encode() + b'\0'
    n = len(ptrs)
    frame = struct.pack('<Q', n) + b''.join(struct.pack('<Q', p) for p in ptrs) + struct.pack('<QQQQQQ', 0, 0, 6, 4096, 0, 0)
    sp = (base - len(frame)) & ~0xF
    uc.mem_write(sp, frame)
    uc.mem_write(base, strings)
    uc.reg_write(UC_X86_REG_RSP, sp)

    state = {'brk': HEAP_BASE, 'mapped': HEAP_BASE, 'status': None, 'ms': 0}
    # An in-memory filesystem with the semantics of docs/spec/syscalls.md: absolute paths,
    # SYS_OPEN fds are read-only, SYS_CREATE is create-if-absent and returns a writable fd,
    # no truncate or append, SYS_UNLINK removes regular files only.
    fs = {'/': None}  # path -> bytearray for files, None for directories
    fds = {}          # fd -> [path, position, writable]
    for extra in (os.environ.get('LYTHOS_EMU_FILES') or '').split(','):
        if '=' in extra:
            p, c = extra.split('=', 1)
            fs[p] = bytearray(c.encode())

    def parent_of(p):
        return p.rsplit('/', 1)[0] or '/'

    def newfd(path, writable):
        fd = next(i for i in range(256) if i not in fds)
        fds[fd] = [path, 0, writable]
        return fd

    def path_arg(ptr, n):
        return bytes(uc.mem_read(ptr, n)).decode()

    def fs_call(nr, a1, a2, a3, a4):
        if nr == 22:
            p = path_arg(a1, a2)
            if p not in fs: return ERR(5)
            if fs[p] is None: return ERR(15)
            return newfd(p, False)
        if nr == 28:
            p = path_arg(a1, a2)
            if p in fs: return ERR(11)
            par = parent_of(p)
            if par not in fs: return ERR(5)
            if fs[par] is not None: return ERR(8)
            fs[p] = bytearray()
            return newfd(p, True)
        if nr == 23:
            f = fds.get(a1)
            if f is None: return ERR(6)
            data = bytes(fs[f[0]][f[1]:f[1] + a3])
            uc.mem_write(a2, data)
            f[1] += len(data)
            return len(data)
        if nr == 24:
            f = fds.get(a1)
            if f is None or not f[2]: return ERR(6)
            data = bytes(uc.mem_read(a2, a3))
            buf = fs[f[0]]
            buf[f[1]:f[1] + len(data)] = data
            f[1] += len(data)
            return len(data)
        if nr == 25:
            return 0 if fds.pop(a1, None) is not None else ERR(6)
        if nr == 26:
            p = path_arg(a1, a2)
            if p not in fs: return ERR(5)
            size = len(fs[p]) if fs[p] is not None else 0
            uc.mem_write(a3, struct.pack('<QQQIIIIH6x', size, 0, 0, 0, 0, 0, 1, 0o644))
            return 0
        if nr == 27:
            p = path_arg(a1, a2)
            if p not in fs: return ERR(5)
            if fs[p] is not None: return ERR(8)
            prefix = p.rstrip('/') + '/'
            names = sorted(q[len(prefix):] for q in fs if q != p and q.startswith(prefix) and '/' not in q[len(prefix):])
            names = names[:a4 // 264]
            for i, nm in enumerate(names):
                e = struct.pack('<IBBH', i + 1, 1, len(nm), 0) + nm.encode().ljust(256, b'\0')
                uc.mem_write(a3 + i * 264, e)
            return len(names)
        if nr == 29:
            p = path_arg(a1, a2)
            if p not in fs: return ERR(5)
            if fs[p] is None: return ERR(4)
            del fs[p]
            return 0
        if nr == 32:
            p = path_arg(a1, a2)
            if p in fs: return ERR(11)
            par = parent_of(p)
            if par not in fs: return ERR(5)
            if fs[par] is not None: return ERR(8)
            fs[p] = None
            return 0
        if nr == 36:
            f = fds.get(a1)
            if f is None: return ERR(6)
            off = a2 - (1 << 64) if a2 >= (1 << 63) else a2
            f[1] = off if a3 == 0 else f[1] + off if a3 == 1 else len(fs[f[0]]) + off
            return f[1]
        return None

    ipc = {'eps': {}, 'names': {}, 'next': 1}

    def ipc_call(nr, a1, a2, a3, a4):
        if nr == 8:
            h = ipc['next']; ipc['next'] += 1
            ipc['eps'][h] = []
            return h
        if nr == 41:
            name = bytes(uc.mem_read(a1, a2))
            if name not in ipc['names']:
                return ERR(5)
            h = ipc['next']; ipc['next'] += 1
            ipc['eps'][h] = ipc['eps'][ipc['names'][name]]
            return h
        ring = ipc['eps'].get(a1)
        if ring is None:
            return ERR(2)
        if nr == 40:
            name = bytes(uc.mem_read(a2, a3))
            if name in ipc['names']:
                return ERR(1)
            ipc['names'][name] = a1
            return 0
        if nr in (6, 12, 43):
            if a3 > 64:
                return ERR(4)
            if len(ring) >= 63:
                if nr == 43:
                    state['ms'] += a4
                    return ERR(7)
                raise SystemExit('deadlock: send on a full ring')
            cap = None
            if nr == 12:
                if a4 not in ipc['eps']:
                    return ERR(2)
                cap = a4
            ring.append((bytes(uc.mem_read(a2, a3)), cap))
            return 0
        if nr in (7, 13, 39, 42):
            if not ring:
                if nr == 39:
                    return ERR(7)
                if nr == 42:
                    state['ms'] += a4
                    return ERR(7)
                raise SystemExit('deadlock: receive on an empty ring')
            data, cap = ring.pop(0)
            n = min(len(data), a3)
            uc.mem_write(a2, data[:n])
            if nr == 13:
                if cap is None:
                    uc.mem_write(a4, (2**64 - 1).to_bytes(8, 'little'))
                else:
                    h = ipc['next']; ipc['next'] += 1
                    ipc['eps'][h] = ipc['eps'][cap]
                    uc.mem_write(a4, h.to_bytes(8, 'little'))
            return n
        return ERR(1)

    out = bytearray()

    def syscall(uc, _):
        nr = uc.reg_read(UC_X86_REG_RAX)
        a1, a2, a3, a4 = (uc.reg_read(r) for r in (UC_X86_REG_RDI, UC_X86_REG_RSI, UC_X86_REG_RDX, UC_X86_REG_R10))
        ret = ERR(1)  # ENOSYS
        if nr == 1:
            state['status'] = a1 & 0xFF
            uc.emu_stop()
            return
        elif nr == 11:
            out.extend(bytes(uc.mem_read(a1, a2)))
            ret = 0
        elif nr in (22, 23, 24, 25, 26, 27, 28, 29, 32, 36):
            ret = fs_call(nr, a1, a2, a3, a4)
        elif nr in (6, 7, 8, 12, 13, 39, 40, 41, 42, 43):
            ret = ipc_call(nr, a1, a2, a3, a4)
        elif nr == 10:
            r8, r9 = uc.reg_read(UC_X86_REG_R8), uc.reg_read(UC_X86_REG_R9)
            caps = [int.from_bytes(bytes(uc.mem_read(a3 + 8 * i, 8)), 'little') for i in range(a4)]
            argv = bytes(uc.mem_read(r8, r9)).split(b'\0')[:-1] if r9 else []
            state['exec'] = (a2, caps, argv)
            ret = 40 + len(caps) * 10 + len(argv) * 100 + sum(len(a) for a in argv) * 1000 + sum(caps) * 100000
        elif nr == 31:
            ret = ERR(5) if a1 in state.setdefault('reaped', set()) else 7
            state['reaped'].add(a1)
        elif nr == 16:
            ret = 3 if a1 == 41 else 0
        elif nr in (0, 5, 19):
            ret = 0
        elif nr == 4:
            ret = a1 + 100
        elif nr == 18:
            ret = 4096
        elif nr in (2, 3):
            if nr == 2:
                if a1 % PAGE or a1 < 0x40000000 or HEAP_BASE <= a1 < HEAP_BASE + (4 << 30) or a1 in state.setdefault('pages', set()):
                    ret = ERR(4)
                else:
                    uc.mem_map(a1, PAGE)
                    state['pages'].add(a1)
                    ret = 0
            else:
                if a1 in state.setdefault('pages', set()):
                    uc.mem_unmap(a1, PAGE)
                    state['pages'].discard(a1)
                    ret = 0
                else:
                    ret = ERR(4)
        elif nr == 62:
            ret = 0x1AF41000 + a2
        elif nr in (65, 66):
            ret = 0
        elif nr == 38:
            if a1 != 0:
                top = (a1 + PAGE - 1) & ~(PAGE - 1)
                if a1 < HEAP_BASE or top > HEAP_BASE + (4 << 30):
                    ret = ERR(4)
                    uc.reg_write(UC_X86_REG_RAX, ret)
                    return
                if top > state['mapped']:
                    uc.mem_map(state['mapped'], top - state['mapped'])
                    state['mapped'] = top
                state['brk'] = a1
            ret = state['brk']
        elif nr in (15, 44):
            ret = state['ms'] + 1_700_000_000_000 * (nr == 44)
        elif nr == 33:
            state['ms'] += (a1 + 999_999) // 1_000_000
            ret = 0
        elif nr == 14:
            ret = 0
        uc.reg_write(UC_X86_REG_RAX, ret)

    uc.hook_add(UC_HOOK_INSN, syscall, None, 1, 0, UC_X86_INS_SYSCALL)
    try:
        uc.emu_start(entry, 0, timeout=60 * UC_SECOND_SCALE)
    except UcError as e:
        sys.stdout.buffer.write(bytes(out))
        sys.stderr.write(f'fault: {e} at rip={uc.reg_read(UC_X86_REG_RIP):#x}\n')
        sys.exit(139)
    sys.stdout.buffer.write(bytes(out))
    if state['status'] is None:
        sys.stderr.write('program did not exit\n')
        sys.exit(1)
    sys.exit(state['status'])

main()
