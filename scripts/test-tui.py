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
                elif token in ('\x1b[2J', '\x1b[?1049h'):
                    # Entering the alternate screen starts from a blank one too: what was printed
                    # before (a lock that is only partly enforced says so) isn't on it.
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
    def program(self):
        """The running program. On Linux what was started is its supervisor, which stays outside
        the lock to start it again, and the program is its one child; elsewhere it is the program."""
        if not sys.platform.startswith('linux'):
            return self.proc.pid
        deadline = time.monotonic() + 15
        while True:
            try:
                children = Path(f'/proc/{self.proc.pid}/task/{self.proc.pid}/children').read_text().split()
            except OSError:
                children = []
            if children:
                return int(children[0])
            if time.monotonic() > deadline:
                raise AssertionError(('no program under the supervisor', self.output[-2000:]))
            time.sleep(.05)
    def locked(self):
        """On Linux the program is locked while it runs: seccomp filters it. Returns its pid."""
        pid = self.program()
        if sys.platform.startswith('linux'):
            status = Path(f'/proc/{pid}/status').read_text()
            assert 'Seccomp:\t2' in status, status
        return pid
    def ended(self, needle):
        """The program ends by itself, saying `needle`, and leaves the terminal as it was."""
        deadline = time.monotonic() + 15
        while self.proc.poll() is None or needle not in self.output:
            if time.monotonic() > deadline:
                raise AssertionError((needle, self.output[-2000:]))
            if select.select([self.master], [], [], .1)[0]:
                try:
                    self.output += os.read(self.master, 65536)
                except OSError:
                    break
        assert self.proc.wait(timeout=5) == 0
        assert termios.tcgetattr(self.slave) == self.before, 'terminal settings not restored'
    def quit(self):
        self.send(b'\x03')
        # Keep reading, as a terminal does: an unread pty (small on macOS) blocks the app's
        # last writes, and it never gets to exit.
        deadline = time.monotonic() + 10
        while self.proc.poll() is None:
            if time.monotonic() > deadline:
                raise AssertionError(('did not exit after ^C', self.screen().decode()))
            if select.select([self.master], [], [], .1)[0]:
                try:
                    self.output += os.read(self.master, 65536)
                except OSError:
                    break
        assert self.proc.wait(timeout=5) == 0
        assert termios.tcgetattr(self.slave) == self.before, 'terminal settings not restored'
    def close(self):
        if self.proc.poll() is None:
            # The program first: the supervisor, killed, would leave it running.
            if sys.platform.startswith('linux'):
                try:
                    os.kill(self.program(), 9)
                except (AssertionError, OSError):
                    pass
            self.proc.kill()
            self.proc.wait()
        os.close(self.master)
        os.close(self.slave)

# Beside the binary, not in the system's temporary folder: the locked program may write there (an
# activation's cookie jar, a Kobo's snapshot), so what it reaches here it reaches as named.
with tempfile.TemporaryDirectory(prefix='crossload-tui-', dir=binary.parent.parent) as tmp:
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
    # In a folder of its own: the settings folder is reachable, and the import folder isn't in it.
    setup_config = root / 'settings' / 'setup.json'
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
        # The reader address is offered ready to use.
        t.expect(b'crosspoint.local')
        # Edit the import folder using Ctrl+U, then point the reader at a port
        # nothing answers on, so this test never reaches a reader that happens
        # to be on the network. Then save with Ctrl+S from where we stand,
        # rather than walking down to the last row.
        first = t.locked()
        t.send(b'\t\r\x15' + str(root / 'setup-imports').encode() + b'\r')
        t.send(b'\t\r\x15127.0.0.1:9\r')
        # Saving starts the interface again with the saved folders. On Linux the lock, made for
        # what was opened at the start, can't take the new import folder and reader, so the
        # program starts again, locked afresh. Elsewhere nothing is locked, and it starts again
        # in the same run.
        t.send(b'\x13')
        t.expect(b'Library ready.')
        if sys.platform.startswith('linux'):
            assert t.locked() != first
        assert b'Start crossload again' not in t.output
        t.quit()
        saved = json.loads(setup_config.read_text())
        assert saved['browse'] == str(books)
        assert saved['output'] == str(root / 'setup-imports')
        assert saved['reader'] == '127.0.0.1:9'
        # Nothing is imported yet. On Linux the restart made the import folder, so its lock covers
        # it; in the same run (elsewhere) it is made on the first import.
        if sys.platform.startswith('linux'):
            assert not any((root / 'setup-imports').iterdir())
        else:
            assert not (root / 'setup-imports').exists()
    finally:
        t.close()
    # Started again, it opens what the settings name.
    t = Terminal(setup_args)
    try:
        t.expect(b'Library ready.')
        t.locked()
        # Saving what the lock already covers starts it again in the same run.
        t.send(b',')
        t.expect(b'Test reader connection')
        t.send(b'\x13')
        time.sleep(1.5)
        assert t.proc.poll() is None, t.output[-2000:]
        assert b'Start crossload again' not in t.output
        t.expect(b'Library ready.')
        t.locked()
        t.send(b'?')
        t.expect(b'Sort by title, then author, then series')
        t.send(b'\x1b')
        t.expect(b'Library ready.')
        t.send(b'f')
        t.expect(b'6  Unreadable')
        t.send(b'4')
        t.expect(b'Missing from CrossPoint')
        t.send(b's')
        t.expect('author ↑'.encode())
        t.send(b',')
        t.expect(b'Test reader connection')
        # The Kobo row takes a typed path with e. Folders are opened when the
        # interface starts, so a typed one is checked then. What Enter lists
        # there depends on what is plugged in, so the unit tests own that half.
        t.send(b'\x1b[B\x1b[Be\x15/etc\r')
        t.expect(b'checked when saved')
        t.send(b'\x1b[A' * 3)
        # A books folder that is not a path is refused, and the complaint
        # arrives at the row that caused it.
        t.send(b'\r\x15unsaved\r')
        t.expect('\u2717 must start with / or ~/'.encode())
        t.send(b'\x13')
        t.expect(b'Books folder: must start with / or ~/')
        t.send(b'\x1b')
        t.expect(b'Library ready.')
        assert json.loads(setup_config.read_text()) == saved
        # A setting the command line also gave keeps the command line's value after a restart,
        # and the panel says so rather than letting a save look like it changed it.
        t.send(b',')
        t.expect(b'Test reader connection')
        t.send(b'\r\x15/elsewhere\r')
        t.expect(b"the command line's applies")
        t.send(b'\x1b[A')
        t.expect(b'Started with --browse; that still applies after the restart.')
        t.send(b'\x1b')
        t.expect(b'Library ready.')
        t.quit()
    finally:
        t.close()
    # Without --browse, the books folder is the saved one. Changed to a folder outside the lock,
    # it starts again by itself (on Linux under a lock made afresh) and shows the books there.
    more = root / 'more-books'
    more.mkdir()
    with zipfile.ZipFile(more / 'Other.epub', 'w') as z:
        for name, data in {
            'mimetype': 'application/epub+zip',
            'META-INF/container.xml': "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>",
            'book.opf': "<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>Other Book</dc:title><dc:creator>Other Author</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>",
            'chapter.xhtml': '<html><head/><body><p>Another synthetic book.</p></body></html>',
        }.items():
            z.writestr(name, data)
    t = Terminal(['--config', str(setup_config), 'tui'])
    try:
        t.expect(b'Library ready.')
        t.expect(b'Test Book')
        first = t.locked()
        t.send(b',')
        t.expect(b'Test reader connection')
        t.send(b'\r\x15' + str(more).encode() + b'\r')
        t.expect(b'checked when saved')
        t.send(b'\x13')
        t.expect(b'Other Book')
        t.expect(b'Library ready.')
        if sys.platform.startswith('linux'):
            assert t.locked() != first
        assert b'Start crossload again' not in t.output
        assert json.loads(setup_config.read_text())['browse'] == str(more)
        t.quit()
    finally:
        t.close()
    args = ['--config', str(root / 'config.json'), 'tui', '--browse', str(books),
            '--output', str(root / 'imports'), '--copy-to', str(card)]
    t = Terminal(args)
    try:
        t.expect(b'Library ready.')
        t.locked()
        t.send(b'/Test\r\r')
        t.expect(b'Copy to')
        assert not (card / 'Test Author/Test Book.epub').exists()
        t.send(b'3')
        t.expect(b'verified')
        assert (card / 'Test Author/Test Book.epub').read_bytes() == original
        assert source.read_bytes() == original
        t.send(b'r')
        # Presence matrix: original on Local, absent on Kobo, device copy on CrossPoint.
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
print("TUI PTY passed: locked (Linux), setup/settings and restart (by itself under a new lock), the command line's settings kept, help/filter/sort, browse/search, verified copy, resize, error and terminal restoration.")
