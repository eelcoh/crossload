"""A CrossPoint reader in File Transfer mode, on 127.0.0.1: its HTTP endpoints on port 80 and its
upload WebSocket on port 81, as spec/src/reader.rs simulates them. For the locked program's test,
run inside a network namespace of its own (those ports need it).

    python3 fake_crosspoint.py OUT

Writes OUT/ready once it listens, and OUT/files.json (each file's path and SHA-256) after every
change to the card.
"""
import base64
import hashlib
import json
import os
import socket
import socketserver
import struct
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

OUT = sys.argv[1]
files, dirs, lock = {}, {'/'}, threading.Lock()


def parent(path):
    head = path.rsplit('/', 1)[0]
    return head or '/'


def join(folder, name):
    return folder.rstrip('/') + '/' + name


def changed():
    listing = {path: hashlib.sha256(data).hexdigest() for path, data in files.items()}
    with open(os.path.join(OUT, 'files.json.part'), 'w') as f:
        json.dump(listing, f)
    os.replace(os.path.join(OUT, 'files.json.part'), os.path.join(OUT, 'files.json'))


class Http(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, body):
        body = body.encode() if isinstance(body, str) else body
        self.send_response(status)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urllib.parse.urlsplit(self.path)
        query = dict(urllib.parse.parse_qsl(url.query))
        with lock:
            if url.path == '/api/status':
                return self.reply(200, '{"version":"1.6.0","device":"X4"}')
            if url.path == '/api/files':
                folder = query.get('path', '/')
                entries = [{'name': d.rsplit('/', 1)[1], 'size': 0, 'isDirectory': True}
                           for d in sorted(dirs) if d != '/' and parent(d) == folder]
                entries += [{'name': p.rsplit('/', 1)[1], 'size': len(b), 'isDirectory': False}
                            for p, b in sorted(files.items()) if parent(p) == folder]
                return self.reply(200, json.dumps(entries))
            if url.path == '/download' and query.get('path') in files:
                return self.reply(200, files[query['path']])
        self.reply(404, 'Not found')

    def do_POST(self):
        form = dict(urllib.parse.parse_qsl(
            self.rfile.read(int(self.headers.get('Content-Length', 0))).decode()))
        with lock:
            if self.path == '/mkdir':
                dirs.add(join(form['path'], form['name']))
                return self.reply(200, 'Folder created: ' + form['name'])
            if self.path == '/rename':
                to = join(parent(form['path']), form['name'])
                if to in files:
                    return self.reply(400, 'File already exists')
                if form['path'] not in files:
                    return self.reply(404, 'Not found')
                files[to] = files.pop(form['path'])
                changed()
                return self.reply(200, 'Renamed successfully')
        self.reply(404, 'Not found')


class Upload(socketserver.BaseRequestHandler):
    def read(self, n):
        data = b''
        while len(data) < n:
            chunk = self.request.recv(n - len(data))
            if not chunk:
                raise EOFError
            data += chunk
        return data

    def send(self, text):
        data = text.encode()
        head = bytes([0x81, len(data)]) if len(data) < 126 else struct.pack('!BBH', 0x81, 126, len(data))
        self.request.sendall(head + data)

    def handle(self):
        request = b''
        while b'\r\n\r\n' not in request:
            request += self.request.recv(4096)
        key = next(line.split(b':', 1)[1].strip() for line in request.split(b'\r\n')
                   if line.lower().startswith(b'sec-websocket-key:'))
        accept = base64.b64encode(hashlib.sha1(key + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
        self.request.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n'
                             b'Connection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
        upload = None
        try:
            while True:
                first, second = self.read(2)
                opcode, size = first & 0x0f, second & 0x7f
                if size == 126:
                    size = struct.unpack('!H', self.read(2))[0]
                elif size == 127:
                    size = struct.unpack('!Q', self.read(8))[0]
                mask = self.read(4) if second & 0x80 else b'\0\0\0\0'
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(self.read(size)))
                if opcode == 8:
                    return
                if opcode == 1:
                    _, name, total, folder = payload.decode().split(':', 3)
                    upload = [join(folder, name), int(total), b'']
                    self.send('READY')
                elif opcode == 2:
                    upload[2] += payload
                    received = len(upload[2])
                    if received % 65536 == 0 or received == upload[1]:
                        self.send(f'PROGRESS:{received}:{upload[1]}')
                    if received == upload[1]:
                        with lock:
                            files[upload[0]] = upload[2]
                            changed()
                        self.send('DONE')
        except EOFError:
            pass


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


http = ThreadingHTTPServer(('127.0.0.1', 80), Http)
ws = Server(('127.0.0.1', 81), Upload)
for server in (http, ws):
    threading.Thread(target=server.serve_forever, daemon=True).start()
open(os.path.join(OUT, 'ready'), 'w').close()
threading.Event().wait()
