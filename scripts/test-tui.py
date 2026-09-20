#!/usr/bin/env python3
"""PTY smoke test with synthetic EPUBs; runs on Linux and macOS (no reader needed)."""
import fcntl
import json
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
    # Setup works without --output and saves only after its explicit action.
    setup_config = root / 'setup.json'
    setup_args = ['--config', str(setup_config), 'tui', '--browse', str(books)]
    t = Terminal(setup_args)
    try:
        t.expect(b'Welcome to Crossload')
        assert not setup_config.exists()
        # Nothing is scanned until setup is answered, and the selected row
        # explains itself rather than leaving the label to do it alone.
        t.expect(b'Local not scanned')
        t.expect(b'Required. Where your own books live')
        t.expect('\u2713 folder found'.encode())
        # Edit the import folder using Ctrl+U, then save with Ctrl+S from
        # anywhere rather than walking down to the last row.
        t.send(b'\t\r\x15' + str(root / 'setup-imports').encode() + b'\r')
        t.send(b'\x13')
        t.expect(b'Library ready.')
        saved = json.loads(setup_config.read_text())
        assert saved['browse'] == str(books)
        assert saved['output'] == str(root / 'setup-imports')
        assert saved['reader'] == 'crosspoint.local'
        assert not (root / 'setup-imports').exists()
        t.send(b'?')
        t.expect(b'Toggle sorting')
        t.send(b'\x1b')
        t.expect(b'Library ready.')
        t.send(b'f')
        t.expect(b'6  Unreadable')
        t.send(b'4')
        t.expect(b'Missing from Xteink')
        t.send(b's')
        t.expect('author ↑'.encode())
        t.send(b',')
        t.expect(b'Test reader connection')
        # The Kobo row takes a typed path with e, and judges it as a Kobo.
        # What Enter detects there depends on what is plugged in, so the unit
        # tests own that half.
        t.send(b'\x1b[B\x1b[Be\x15/etc\r')
        t.expect('\u26a0 no Kobo database here'.encode())
        t.send(b'\x1b[A' * 3)
        # A books folder that does not exist is refused, and the complaint
        # arrives at the row that caused it.
        t.send(b'\r\x15/unsaved\r')
        t.expect('\u2717 not found'.encode())
        t.send(b'\x13')
        t.expect(b'Books folder: not found')
        t.send(b'\x1b')
        t.expect(b'Library ready.')
        assert json.loads(setup_config.read_text()) == saved
        t.quit()
    finally:
        t.close()
    args = ['--config', str(root / 'config.json'), 'tui', '--browse', str(books),
            '--output', str(root / 'imports'), '--copy-to', str(card)]
    t = Terminal(args)
    try:
        t.expect(b'Library ready.')
        t.send(b'/Test\r\r')
        t.expect(b'Copy to')
        assert not (card / 'Test Author/Test Book.epub').exists()
        t.send(b'3')
        t.expect(b'verified')
        assert (card / 'Test Author/Test Book.epub').read_bytes() == original
        assert source.read_bytes() == original
        t.send(b'r')
        # Presence matrix: original on Local, absent on Kobo, device copy on Xteink.
        t.expect('● · ◐'.encode())
        t.resize(3, 12)
        t.send(b'r')
        time.sleep(.2)
        t.resize(28, 120)
        t.send(b'r')
        t.expect(b'Library ready.')
        t.quit()
    finally:
        t.close()
    # Failure must be displayed and leave no published output, with normal quit.
    source.write_bytes(b'not an EPUB')
    t = Terminal(args)
    try:
        t.expect(b'Library ready.')
        t.send(b'/Test.epub\r\r3')
        t.expect(b'Error:')
        t.quit()
        assert (card / 'Test Author/Test Book.epub').read_bytes() == original
    finally:
        t.close()
print('TUI PTY passed: setup/settings, help/filter/sort, browse/search, verified copy, resize, error and terminal restoration.')
