"""Local-only test fixture; no user credentials or remote requests."""
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
from pathlib import Path
import os

ROOT = Path(os.environ['SHANDIAN_TEST_STREAM_DIR']).resolve()
class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)
    def translate_path(self, path):
        path = urlsplit(path).path
        if path == '/protected/stream': return str(ROOT / 'encrypted.m3u8')
        if path.startswith('/protected/'): return str(ROOT / path.removeprefix('/protected/'))
        return super().translate_path(path)
    def guess_type(self, path):
        if self.path.split('?', 1)[0] == '/protected/stream': return 'application/vnd.apple.mpegurl'
        return super().guess_type(path)
    def do_HEAD(self):
        if self.path.startswith('/protected/') and 'fixture=allowed' not in self.headers.get('Cookie', ''):
            self.send_error(403); return
        super().do_HEAD()
    def do_GET(self):
        if self.path.startswith('/protected/') and 'fixture=allowed' not in self.headers.get('Cookie', ''):
            self.send_error(403); return
        super().do_GET()
    def log_message(self, *args): pass
ThreadingHTTPServer(('127.0.0.1', 18779), Handler).serve_forever()
