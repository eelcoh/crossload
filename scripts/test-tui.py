#!/usr/bin/env python3
"""PTY smoke test with synthetic EPUBs; runs on Linux and macOS (no reader needed)."""
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
import zipfile

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/crossload').resolve()

class Terminal:
    def __init__(self, args):
        self.master, self.slave = pty.openpty()
        self.resize(28, 120)
        self.before = termios.tcgetattr(self.slave)
        self.proc = subprocess.Popen([str(binary), *args], stdin=self.slave, stdout=self.slave,
                                     stderr=self.slave, start_new_session=True)
        self.output = b''
    def resize(self, rows, cols):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
    def send(self, data):
        os.write(self.master, data)
    def screen(self):
        # Reconstruct cursor-positioned output, including characters retained by
        # Ratatui's differential renderer between frames.
        cells = {}
        row = col = 1
        for token in re.findall(r'\x1b\[[0-?]*[ -/]*[@-~]|[^\x1b]', self.output.decode('utf-8', errors='replace')):
            if token.startswith('\x1b['):
                if token[-1] in 'Hf':
                    values = token[2:-1].split(';')
                    row = int(values[0] or 1)
                    col = int(values[1] or 1) if len(values) > 1 else 1
                elif token == '\x1b[2J':
                    cells.clear()
            elif token == '\r':
                col = 1
            elif token == '\n':
                row += 1
            else:
                cells[row, col] = token
                col += 1
        return '\n'.join(''.join(cells.get((r, c), ' ') for c in range(1, 121))
                         for r in range(1, 40)).encode()
    def expect(self, needle):
        deadline = time.monotonic() + 15
        while needle not in self.screen():
            if time.monotonic() > deadline:
                raise AssertionError((needle, self.output[-2000:]))
            if select.select([self.master], [], [], .1)[0]:
                self.output += os.read(self.master, 65536)
    def quit(self):
        self.send(b'\x03')
        assert self.proc.wait(timeout=10) == 0
        assert termios.tcgetattr(self.slave) == self.before, 'terminal settings not restored'
    def close(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()
        os.close(self.master)
        os.close(self.slave)

with tempfile.TemporaryDirectory(prefix='crossload-tui-') as tmp:
    root = Path(tmp)
    books = root / 'books'
    books.mkdir()
    card = root / 'card'
    card.mkdir()
    source = books / 'Test.epub'
    with zipfile.ZipFile(source, 'w') as z:
        for name, data in {
            'mimetype': 'application/epub+zip',
            'META-INF/container.xml': "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>",
            'book.opf': "<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>Test Book</dc:title><dc:creator>Test Author</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>",
            'chapter.xhtml': '<html><head/><body><p>A synthetic book.</p></body></html>',
        }.items():
            z.writestr(name, data)
    original = source.read_bytes()
    args = ['--config', str(root / 'config.json'), 'tui', '--browse', str(books),
            '--output', str(root / 'imports'), '--copy-to', str(card)]
    t = Terminal(args)
    try:
        t.expect(b'Reader checked.')
        t.send(b'/Test\r\r')
        t.expect(b'verified')
        assert (card / 'Test Author/Test Book.epub').read_bytes() == original
        assert source.read_bytes() == original
        t.send(b'r')
        t.expect(b'Present')
        t.resize(3, 12)
        t.send(b'r')
        time.sleep(.2)
        t.resize(28, 120)
        t.send(b'r')
        t.expect(b'Reader checked.')
        t.quit()
    finally:
        t.close()
    # Failure must be displayed and leave no published output, with normal quit.
    source.write_bytes(b'not an EPUB')
    t = Terminal(args)
    try:
        t.expect(b'Reader checked.')
        t.send(b'\r')
        t.expect(b'Error:')
        t.quit()
        assert (card / 'Test Author/Test Book.epub').read_bytes() == original
    finally:
        t.close()
print('TUI PTY passed: browse/search, verified copy, resize, error and terminal restoration.')
