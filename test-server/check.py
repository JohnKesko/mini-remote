#!/usr/bin/env python3
"""Quick health check for the OpenVNC test server (container "openvnc-testpi").

For each VNC port: read the RFB banner, answer "RFB 003.008", list the security
types the server offers. On the no-login port it also finishes the handshake and
prints the screen size and desktop name. Also checks that SSH answers.
Standard library only. Exit code 0 = everything as expected.
"""
import socket
import struct
import sys

HOST = "127.0.0.1"
TIMEOUT = 5

SECURITY_NAMES = {
    1: "None",
    2: "VNC auth",
    5: "RSA-AES",
    6: "RSA-AES unencrypted",
    16: "Tight",
    19: "VeNCrypt",
    30: "Apple DH",
    129: "RSA-AES-256",
    130: "RSA-AES-256 unencrypted",
}

VENCRYPT_NAMES = {
    256: "Plain", 257: "TLSNone", 258: "TLSVnc", 259: "TLSPlain",
    260: "X509None", 261: "X509Vnc", 262: "X509Plain",
    263: "TLSSasl", 264: "X509Sasl",
}


def recv_exact(sock, n):
    data = b""
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise ConnectionError("connection closed after %d of %d bytes" % (len(data), n))
        data += chunk
    return data


def name_of(t):
    return "%d %s" % (t, SECURITY_NAMES.get(t, "unknown"))


def vencrypt_subtypes(port):
    """Open a fresh connection, pick VeNCrypt and list its sub-types."""
    with socket.create_connection((HOST, port), timeout=TIMEOUT) as s:
        s.settimeout(TIMEOUT)
        recv_exact(s, 12)
        s.sendall(b"RFB 003.008\n")
        n = recv_exact(s, 1)[0]
        recv_exact(s, n)
        s.sendall(bytes([19]))
        major, minor = recv_exact(s, 2)
        s.sendall(bytes([0, 2]) if (major, minor) >= (0, 2) else bytes([0, 1]))
        if recv_exact(s, 1)[0] != 0:
            return None
        count = recv_exact(s, 1)[0]
        return [struct.unpack(">I", recv_exact(s, 4))[0] for _ in range(count)]


def check_vnc(port, label, expect_any, expect_all=(), finish_handshake=False):
    """Returns True when the server offers what we expect."""
    print("\n== port %d: %s ==" % (port, label))
    ok = True
    try:
        with socket.create_connection((HOST, port), timeout=TIMEOUT) as s:
            s.settimeout(TIMEOUT)
            banner = recv_exact(s, 12)
            print("  banner:   %r" % banner.decode("ascii", "replace"))
            if not banner.startswith(b"RFB "):
                print("  FAIL: not an RFB banner")
                return False
            s.sendall(b"RFB 003.008\n")
            count = recv_exact(s, 1)[0]
            if count == 0:
                (rlen,) = struct.unpack(">I", recv_exact(s, 4))
                print("  FAIL: server refused: %s" % recv_exact(s, rlen).decode("utf-8", "replace"))
                return False
            types = list(recv_exact(s, count))
            print("  security types: " + ", ".join(name_of(t) for t in types))

            if expect_any and not any(t in types for t in expect_any):
                print("  FAIL: expected at least one of: " + ", ".join(name_of(t) for t in expect_any))
                ok = False
            for t in expect_all:
                if t not in types:
                    print("  FAIL: expected %s to be offered" % name_of(t))
                    ok = False

            if finish_handshake and 1 in types:
                s.sendall(bytes([1]))
                (result,) = struct.unpack(">I", recv_exact(s, 4))
                print("  SecurityResult: %d (%s)" % (result, "OK" if result == 0 else "failed"))
                if result != 0:
                    return False
                s.sendall(bytes([1]))  # ClientInit, shared = 1
                width, height = struct.unpack(">HH", recv_exact(s, 4))
                pixel_format = recv_exact(s, 16)
                (nlen,) = struct.unpack(">I", recv_exact(s, 4))
                desktop = recv_exact(s, nlen).decode("utf-8", "replace")
                print("  screen size:    %dx%d (%d bits per pixel)" % (width, height, pixel_format[0]))
                print("  desktop name:   %r" % desktop)
                if (width, height) != (1920, 1080):
                    print("  FAIL: expected 1920x1080")
                    ok = False
    except Exception as exc:  # noqa: BLE001 - this is a diagnostic script
        print("  FAIL: %s: %s" % (type(exc).__name__, exc))
        return False

    if 19 in types:
        try:
            subs = vencrypt_subtypes(port)
            if subs is not None:
                print("  VeNCrypt sub-types: " + ", ".join(
                    "%d %s" % (x, VENCRYPT_NAMES.get(x, "unknown")) for x in subs))
        except Exception as exc:  # noqa: BLE001
            print("  (could not list VeNCrypt sub-types: %s)" % exc)
    print("  " + ("OK" if ok else "FAILED"))
    return ok


def check_ssh(port):
    print("\n== port %d: SSH / SFTP ==" % port)
    try:
        with socket.create_connection((HOST, port), timeout=TIMEOUT) as s:
            s.settimeout(TIMEOUT)
            banner = s.recv(255).split(b"\n")[0].decode("ascii", "replace").strip()
        print("  banner: %s" % banner)
        if banner.startswith("SSH-2.0"):
            print("  OK")
            return True
        print("  FAIL: expected an SSH-2.0 banner")
    except Exception as exc:  # noqa: BLE001
        print("  FAIL: %s: %s" % (type(exc).__name__, exc))
    return False


def main():
    results = [
        # Like a real Pi: PAM login, expect RSA-AES and/or VeNCrypt.
        check_vnc(5900, "Pi style login (Linux user + password)", expect_any=(5, 129, 19)),
        # Classic VNC password.
        check_vnc(5901, "classic VNC password", expect_any=(), expect_all=(2,)),
        # No login at all: finish the handshake too.
        check_vnc(5902, "no login", expect_any=(), expect_all=(1,), finish_handshake=True),
        check_ssh(2222),
    ]
    print("\n" + ("ALL CHECKS PASSED" if all(results) else "SOME CHECKS FAILED"))
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
