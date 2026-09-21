#!/usr/bin/env python3
"""A static server for development: threaded, keep-alive, no request logging.

python -m http.server speaks HTTP/1.0 and closes the connection after every
response, which is painful when the viewer asks for a few dozen small files at
once.
"""
import functools, os, sys, urllib.request, urllib.error
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

# Paths the backend answers itself; in production Caddy proxies them.
BACKEND = "http://127.0.0.1:8081"
PROXIED = ("/api/v1/geocode", "/api/v1/reverse", "/api/v1/status.json")


class Handler(SimpleHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def end_headers(self):
        # Development only: never let the browser keep a copy, so a reload
        # always shows the latest build. Production caching lives in Caddy.
        if not getattr(self, "_proxy", False):
            self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def do_GET(self):
        if self.path.startswith(PROXIED):
            self._proxy = True
            try:
                with urllib.request.urlopen(BACKEND + self.path, timeout=20) as r:
                    body, status, ctype = r.read(), r.status, r.headers.get("Content-Type", "application/json")
            except urllib.error.HTTPError as e:
                body, status, ctype = e.read(), e.code, "application/json"
            except Exception:
                body, status, ctype = b'{"error":"backend down"}', 502, "application/json"
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self._proxy = False
        super().do_GET()


if __name__ == "__main__":
    root = sys.argv[1] if len(sys.argv) > 1 else os.getcwd()
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 8000
    ThreadingHTTPServer(
        ("127.0.0.1", port), functools.partial(Handler, directory=root)
    ).serve_forever()
