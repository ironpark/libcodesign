#!/usr/bin/env python3
"""Package one built target into the release archive consumers download.

Layout inside the archive -- consumers link lib/libzapp_rcodesign.a against the
declarations in include/zapp_rcodesign.h:

    lib/libzapp_rcodesign.a
    include/zapp_rcodesign.h
    LICENSE NOTICE THIRD_PARTY_LICENSES.html Cargo.lock README.md
"""
import hashlib
import os
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGETS = ("linux_amd64", "linux_arm64", "windows_amd64", "windows_arm64")


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in TARGETS:
        raise SystemExit(f"usage: package.py <{'|'.join(TARGETS)}>")
    target = sys.argv[1]
    version = os.environ.get("LIBCODESIGN_VERSION", "dev").lstrip("v")

    archive = ROOT / "dist" / target / "libzapp_rcodesign.a"
    if not archive.is_file():
        raise SystemExit(f"build {target} first: {archive} is missing")
    notices = ROOT / "THIRD_PARTY_LICENSES.html"
    if not notices.is_file():
        raise SystemExit("generate THIRD_PARTY_LICENSES.html with cargo-about before packaging")

    files = [(archive, "lib/libzapp_rcodesign.a"),
             (ROOT / "include/zapp_rcodesign.h", "include/zapp_rcodesign.h"),
             (notices, "THIRD_PARTY_LICENSES.html")]
    files += [(ROOT / name, name) for name in ("LICENSE", "NOTICE", "Cargo.lock", "README.md")]

    output = ROOT / "release-assets"
    output.mkdir(exist_ok=True)
    bundle = output / f"libcodesign_{version}_{target}.tar.gz"
    with tarfile.open(bundle, "w:gz") as tar:
        for source, name in files:
            tar.add(source, arcname=name)
    digest = hashlib.sha256(bundle.read_bytes()).hexdigest()
    (output / (bundle.name + ".sha256")).write_text(f"{digest}  {bundle.name}\n")
    print(f"{bundle} ({bundle.stat().st_size / 1e6:.0f} MB)\n{digest}")


if __name__ == "__main__":
    main()
