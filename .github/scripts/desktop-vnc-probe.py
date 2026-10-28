"""Exercise real keyboard and pointer input through the authenticated runtime stream."""
import base64
import hashlib
import os
import socket
import struct
import subprocess
import time


class Stream:
    def __init__(self):
        self.socket = socket.create_connection(("127.0.0.1", 9090), timeout=10)
        self.socket.settimeout(10)
        key = base64.b64encode(os.urandom(16)).decode()
        token = os.environ["CMA_DESKTOP_TEST_TOKEN"]
        self.socket.sendall(("GET /runtime/desktop HTTP/1.1\r\nHost: localhost\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"
            f"Sec-WebSocket-Key: {key}\r\nx-cma-runtime-token: {token}\r\n\r\n").encode())
        headers = bytearray()
        while not headers.endswith(b"\r\n\r\n"):
            headers.extend(self.raw(1))
            assert len(headers) < 16384
        assert headers.startswith(b"HTTP/1.1 101 "), headers.splitlines()[0]
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
        assert expected in headers
        self.buffer = bytearray()

    def raw(self, count):
        result = bytearray()
        while len(result) < count:
            chunk = self.socket.recv(count - len(result))
            assert chunk, "desktop stream closed early"
            result.extend(chunk)
        return bytes(result)

    def send(self, data, opcode=2):
        mask = os.urandom(4)
        length = len(data)
        assert length < 65536
        header = bytes([0x80 | opcode]) + (bytes([0x80 | length]) if length < 126 else b"\xfe" + struct.pack("!H", length))
        self.socket.sendall(header + mask + bytes(v ^ mask[i % 4] for i, v in enumerate(data)))

    def read(self, count):
        while len(self.buffer) < count:
            first, second = self.raw(2)
            length = second & 127
            if length == 126:
                length = struct.unpack("!H", self.raw(2))[0]
            elif length == 127:
                length = struct.unpack("!Q", self.raw(8))[0]
            assert not second & 128 and length <= 8 * 1024 * 1024
            payload = self.raw(length)
            opcode = first & 15
            if opcode == 9:
                self.send(payload, 10)
            else:
                assert opcode in (0, 2), "unexpected desktop frame"
                self.buffer.extend(payload)
        data = self.buffer[:count]
        del self.buffer[:count]
        return bytes(data)

    def key(self, keysym, down):
        self.send(struct.pack("!BBHI", 4, int(down), 0, keysym))

    def click(self, x, y):
        self.send(struct.pack("!BBHH", 5, 1, x, y))
        self.send(struct.pack("!BBHH", 5, 0, x, y))


def main():
    stream = Stream()
    try:
        assert stream.read(12) == b"RFB 003.008\n"
        stream.send(b"RFB 003.008\n")
        count = stream.read(1)[0]
        assert count and 1 in stream.read(count)
        stream.send(b"\x01")
        assert stream.read(4) == b"\0\0\0\0"
        stream.send(b"\x01")
        header = stream.read(24)
        width, height = struct.unpack("!HH", header[:4])
        assert (width, height) == (1280, 800)
        name_length = struct.unpack("!I", header[20:])[0]
        assert name_length < 4096
        stream.read(name_length)
        # Request one uncompressed full frame to prove pixels cross both relays.
        stream.send(struct.pack("!BBHi", 2, 0, 1, 0))
        stream.send(struct.pack("!BBHHHH", 3, 0, 0, 0, width, height))
        update = stream.read(4)
        assert update[0] == 0
        rectangles = struct.unpack("!H", update[2:])[0]
        assert rectangles > 0
        pixels = 0
        for _ in range(rectangles):
            _, _, w, h, encoding = struct.unpack("!HHHHi", stream.read(12))
            assert encoding == 0
            pixels += len(stream.read(w * h * (header[4] // 8)))
        assert pixels > 100000
        # Viewers request the next update after rendering a frame. x11vnc also
        # uses that request for initial link calibration; omitting it stalls
        # queued input until the calibration timeout despite a healthy relay.
        stream.send(struct.pack("!BBHHHH", 3, 1, 0, 0, width, height))
        stream.click(120, 120)
        stream.key(0xffe3, True)  # Control
        stream.key(ord('a'), True)
        stream.key(ord('a'), False)
        stream.key(0xffe3, False)
        for char in 'cma-user':
            stream.key(ord(char), True)
            stream.key(ord(char), False)
        stream.click(120, 220)
        # A browser connection stays open while the user observes the result.
        # Closing here can tear down a relay before its queued input reaches X.
        deadline = time.monotonic() + 5
        while True:
            title = subprocess.check_output(
                ['xdotool', 'getactivewindow', 'getwindowname'], text=True).strip()
            if title.startswith('cma-ui:cma-user'):
                break
            assert time.monotonic() < deadline, 'stream input did not update browser title: ' + title
            time.sleep(0.1)
        print('authenticated-framebuffer-and-input: pass')
    finally:
        stream.socket.close()


if __name__ == '__main__':
    main()
