import os, pty, sys, time, select, struct, fcntl, termios
def run(keys, path, rows=10, cols=40):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir('/tmp/ly'); os.execv('/tmp/ly/kilo', ['/tmp/ly/kilo', os.path.basename(path)])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
    out = b''
    def drain(t=0.4):
        nonlocal out
        end = time.time()+t
        while time.time() < end:
            r,_,_ = select.select([fd],[],[],0.05)
            if r:
                try: d = os.read(fd, 65536)
                except OSError: return
                if not d: return
                out += d
    drain(0.6)
    for k in keys:
        os.write(fd, k); drain(0.25)
    drain(0.5)
    try:
        _, st = os.waitpid(pid, os.WNOHANG)
    except ChildProcessError: st = None
    return out, st
if __name__ == '__main__':
    path = '/tmp/ly/t.txt'
    open(path,'w').write('hello\nworld\n')
    keys = [b'X', b'\x1b[B', b'\x1b[F', b'!', b'\r', b'ab', b'\x7f', b'\x13', b'\x11']
    out, st = run(keys, path)
    print(repr(open(path).read()), st)
    print(out[-300:])
