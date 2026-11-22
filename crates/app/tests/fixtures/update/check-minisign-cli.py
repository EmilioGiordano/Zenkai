# Signs the fixture zip with the real minisign CLI and the TEST keys, and checks it writes the
# same signatures as make-fixtures.mjs, which the updater tests accept. Ed25519 is deterministic,
# so equal bytes prove `minisign -S -t zenkai/<version>/<platform>` output verifies in the app.
# Run with `python check-minisign-cli.py [path to minisign]` from any folder.
import base64
import hashlib
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ZIP = HERE / "zenkai-v0.2.0-windows-x64.zip"
TEST_KEYS = {
    "current": "RWR2xaKuXEhnqKoTmGepuFVdNFslHlo2RK/tTpOEQP0gPcf3OBdsKMGO",
    "next": "RWTLQ9vx1ajpnR7Ateo1t/yue3+jrifpcJftIpn7oSnjdvQLkO3/lTIy",
}
CASES = [
    ("current", "zenkai/0.2.0/windows-x64", "good.minisig"),
    ("current", "zenkai/0.1.0/windows-x64", "old-version.minisig"),
    ("next", "zenkai/0.2.0/windows-x64", "next-key.minisig"),
]


# The TEST seed is the one make-fixtures.mjs derives; the key id and public half come from the
# key the tests trust. Written unencrypted, as `minisign -G -W` does, so no password is asked.
def secret_key_file(label):
    public = base64.b64decode(TEST_KEYS[label])
    key_id, public_key = public[2:10], public[10:]
    seed = hashlib.sha256(f"zenkai TEST key {label}".encode()).digest()
    secret = seed + public_key
    checksum = hashlib.blake2b(b"Ed" + key_id + secret, digest_size=32).digest()
    body = b"Ed" + b"\0\0" + b"B2" + bytes(32 + 8 + 8) + key_id + secret + checksum
    return f"untrusted comment: zenkai TEST key {label}\n{base64.b64encode(body).decode()}\n"


def signed_lines(path):
    return path.read_bytes().split(b"\n")[1:4]


def main():
    minisign = sys.argv[1] if len(sys.argv) > 1 else "minisign"
    failed = False
    with tempfile.TemporaryDirectory() as work:
        for label, comment, fixture in CASES:
            key = Path(work) / f"{label}.key"
            key.write_bytes(secret_key_file(label).encode())
            signature = Path(work) / fixture
            subprocess.run(
                [minisign, "-S", "-s", key, "-m", ZIP, "-x", signature, "-t", comment],
                check=True,
                stdin=subprocess.DEVNULL,
            )
            same = signed_lines(signature) == signed_lines(HERE / fixture)
            verified = subprocess.run(
                [minisign, "-V", "-Q", "-P", TEST_KEYS[label], "-m", ZIP, "-x", HERE / fixture],
                capture_output=True,
                text=True,
                stdin=subprocess.DEVNULL,
            )
            accepted = verified.returncode == 0 and verified.stdout.strip() == comment
            print(f"{fixture}: same signature {same}, minisign -V accepts it {accepted}")
            failed = failed or not (same and accepted)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
