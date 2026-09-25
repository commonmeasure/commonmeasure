#!/usr/bin/env python3
"""A loopback Content Telemetry receiver for the demonstration arc.

The relay refuses to run without a receiver, and the demonstration's closing
beat is watching exactly which events cross that boundary. This receiver is
the audience's window, not a receiver-product claim: it accepts `POST /events`,
answers 200 with ``{"status": "ok", "events_created": n}`` (the relay takes
any 2xx as acceptance and reads `events_created` when present,
crates/commonmeasure-relay/src/client.rs) and appends every batch it accepts,
verbatim, to an NDJSON file so what arrived can be read after the run. The claim it
supports is about the relay's boundary — what left the machine and under whose
clearance — and the delivered bytes on disk are that claim's evidence. It
asserts nothing about integration with a real receiver.

Usage: receiver.py <port> <log-path>   (listens on 127.0.0.1 only)
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


class Receiver(BaseHTTPRequestHandler):
    log_path = None

    def do_POST(self):
        if self.path != "/events":
            self.answer(404, {"detail": "this receiver accepts POST /events only"})
            return
        length = int(self.headers.get("Content-Length", 0))
        try:
            batch = json.loads(self.rfile.read(length))
        except ValueError:
            self.answer(400, {"detail": "the body was not JSON"})
            return
        events = batch.get("events", [])
        with open(self.log_path, "a", encoding="utf-8") as log:
            log.write(json.dumps(batch) + "\n")
        for event in events:
            print(
                "received: %s %s" % (event.get("type", "?"), event.get("content_url", "?")),
                flush=True,
            )
        self.answer(200, {"status": "ok", "events_created": len(events)})

    def answer(self, status, body):
        encoded = json.dumps(body).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, *arguments):
        # The delivered batches on disk are the record; request-line noise is not.
        pass


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: receiver.py <port> <log-path>")
    Receiver.log_path = sys.argv[2]
    server = HTTPServer(("127.0.0.1", int(sys.argv[1])), Receiver)
    print("receiver listening on 127.0.0.1:%s, logging to %s" % (sys.argv[1], sys.argv[2]), flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
