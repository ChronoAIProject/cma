"""Fixed bounded stdio-to-loopback relay. No credentials or request data in argv."""
import os
import select
import socket
import time

LIMIT = 65536
CONNECT_RETRY_SECONDS = 2.5


def connect():
    deadline = time.monotonic() + CONNECT_RETRY_SECONDS
    delay = 0.05
    while True:
        try:
            return socket.create_connection(("127.0.0.1", 9090), timeout=5)
        except ConnectionRefusedError:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise
            poller = select.poll()
            poller.register(0, select.POLLHUP | select.POLLERR)
            poller.register(1, select.POLLHUP | select.POLLERR)
            events = dict(poller.poll(max(1, int(min(delay, remaining) * 1000))))
            if any(events.get(fd, 0) & (select.POLLHUP | select.POLLERR) for fd in (0, 1)):
                raise OSError("relay caller closed")
            delay = min(0.1, delay * 2)


def relay():
    with connect() as peer:
        # This marker is consumed by CMA before any HTTP bytes or credentials.
        marker = memoryview(b"CMA-EXEC-1\n")
        while marker:
            marker = marker[os.write(1, marker):]
        peer.setblocking(False)
        os.set_blocking(0, False)
        os.set_blocking(1, False)
        incoming = bytearray()
        outgoing = bytearray()
        peer_open = True
        while peer_open or outgoing:
            poller = select.poll()
            # Explicit HUP also works on hosts whose poll ignores events=0.
            # Observe cancellation even while either bounded buffer is full.
            closing = select.POLLHUP | select.POLLERR
            poller.register(0, closing | (select.POLLIN if len(incoming) < LIMIT and peer_open else 0))
            poller.register(1, closing | (select.POLLOUT if outgoing else 0))
            peer_events = select.POLLIN if peer_open and len(outgoing) < LIMIT else 0
            if incoming and peer_open:
                peer_events |= select.POLLOUT
            if peer_events:
                poller.register(peer, peer_events)
            # Some poll implementations do not wake on a newly arrived HUP when
            # only HUP was requested. Recheck within one second under backpressure.
            events = dict(poller.poll(1000))
            if any(events.get(fd, 0) & (select.POLLHUP | select.POLLERR) for fd in (0, 1)):
                return
            if events.get(0, 0) & select.POLLIN:
                data = os.read(0, LIMIT - len(incoming))
                if not data:
                    return  # Caller cancellation must not leave a detached relay.
                incoming.extend(data)
            peer_ready = events.get(peer.fileno(), 0)
            if peer_ready & select.POLLERR:
                raise OSError("relay peer failed")
            if peer_ready & (select.POLLIN | select.POLLHUP) and len(outgoing) < LIMIT:
                data = peer.recv(LIMIT - len(outgoing))
                if data:
                    outgoing.extend(data)
                else:
                    peer_open = False
                    incoming.clear()
            if peer_ready & select.POLLHUP:
                incoming.clear()
            if peer_ready & select.POLLOUT and peer_open and incoming:
                del incoming[:peer.send(incoming)]
            if events.get(1, 0) & select.POLLOUT:
                del outgoing[:os.write(1, outgoing)]


try:
    relay()
except Exception:
    # Neither Python tracebacks nor reflected peer content belong in diagnostics.
    os._exit(70)
