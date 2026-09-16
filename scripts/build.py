#!/usr/bin/env python3
"""Build libzapp_rcodesign.a for one Windows/Linux target into dist/<target>/.

The archive keeps its debug sections unless --strip is given; a release build
strips them, which is what makes the artifact small enough to ship.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIB = "libzapp_rcodesign.a"
# First bytes of a short import member: Sig1 = 0, Sig2 = 0xFFFF.
IMPORT_MAGIC = b"\x00\x00\xff\xff"
TARGETS = {
    "linux_amd64": "x86_64-unknown-linux-gnu",
    "linux_arm64": "aarch64-unknown-linux-gnu",
    "windows_amd64": "x86_64-pc-windows-gnullvm",
    "windows_arm64": "aarch64-pc-windows-gnullvm",
}
# The system libraries the archive needs. Same for every architecture of one OS,
# and listed after the archive so a static link resolves in order.
SYSTEM_LIBS = {
    "linux": ["-ldl", "-lpthread", "-lm"],
    "windows": [
        "-static", "-lws2_32", "-luserenv", "-lbcrypt", "-lntdll", "-lcrypt32",
        "-lncrypt", "-lsecur32", "-liphlpapi", "-lole32", "-loleaut32", "-lruntimeobject",
    ],
}


def run(command, **kwargs):
    print("+", " ".join(str(part) for part in command), flush=True)
    subprocess.run(command, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=TARGETS)
    parser.add_argument("--test", action="store_true", help="run the Rust tests on the target host")
    parser.add_argument("--strip", action="store_true", help="drop debug sections from the archive")
    parser.add_argument("--smoke", action="store_true", help="link and run tests/smoke.c against the archive")
    args = parser.parse_args()
    os.chdir(ROOT)
    goos = args.target.split("_")[0]
    target = TARGETS[args.target]

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    cc = env.get("CC") or ("cc" if goos == "linux" else target.split("-")[0] + "-w64-mingw32-clang")
    env["CC"] = cc
    if goos == "windows":
        env.setdefault("AR", "llvm-ar")
    env["CARGO_TARGET_" + target.upper().replace("-", "_") + "_LINKER"] = cc

    run(["rustup", "target", "add", target])
    cargo_args = ["--locked", "--release", "--target", target]
    run(["cargo", "build", *cargo_args], env=env)
    if args.test:
        # Only the test run pulls the crate's dev-dependencies in.
        run(["cargo", "test", *cargo_args], env=env)

    outdir = ROOT / "dist" / args.target
    outdir.mkdir(parents=True, exist_ok=True)
    archive = outdir / LIB
    shutil.copy2(ROOT / "target" / target / "release" / LIB, archive)
    if args.strip:
        strip_debug(goos, archive, env)
    if args.smoke:
        smoke(goos, cc, archive, outdir, env)
    print(f"{archive} ({archive.stat().st_size / 1e6:.0f} MB)")


def strip_debug(goos, archive, env):
    """Drop the .debug_* sections, which are most of the archive's size. The
    symbol table the linker resolves against, and the code itself, are kept."""
    strip = env.get("STRIP") or ("llvm-strip" if goos == "windows" else "strip")
    if goos != "windows":
        run([strip, "--strip-debug", str(archive)], env=env)
        return
    # llvm-strip refuses a Windows archive as a whole: raw-dylib linkage puts a
    # few short import members in it, which it reports as an unsupported object
    # file format. Strip the objects one by one instead and rebuild the archive,
    # leaving the import members untouched.
    ar = env.get("AR") or "llvm-ar"
    with tempfile.TemporaryDirectory(dir=archive.parent) as tmp:
        members = extract(archive, Path(tmp))
        objects = [m for m in members if m.read_bytes()[:4] != IMPORT_MAGIC]
        for batch in chunks(objects):
            run([strip, "--strip-debug", *batch], env=env)
        # Over a thousand members go back in: append them in batches rather than
        # in one command line Windows would reject, then write the index once.
        archive.unlink()
        for index, batch in enumerate(chunks(members)):
            run([ar, "qcD" if index == 0 else "qD", str(archive), *batch], env=env)
        run([ar, "sD", str(archive)], env=env)


def chunks(members, size=100):
    """Batches of member paths, small enough to pass on a command line."""
    return [[str(m) for m in members[start:start + size]] for start in range(0, len(members), size)]


def extract(archive, into):
    """Unpack a GNU-format archive, returning one path per member in order.

    Members go in a directory of their own because ar stores only the file name
    and the same name can appear more than once. The symbol table and long name
    table are left out: ar writes both again when the archive is rebuilt."""
    members = []
    with archive.open("rb") as f:
        if f.read(8) != b"!<arch>\n":
            raise SystemExit(f"{archive} is not a GNU-format archive")
        names = b""
        while header := f.read(60):
            if len(header) < 60 or header[58:60] != b"`\n":
                raise SystemExit(f"{archive} has a malformed member header")
            name = header[:16].decode().rstrip()
            size = int(header[48:58])
            body = f.read(size)
            f.seek(size % 2, os.SEEK_CUR)  # members are padded to even offsets
            if name == "//":
                names = body
            elif name in ("/", "/SYM64/"):
                continue
            else:
                if name.startswith("/"):  # an offset into the long name table
                    start = int(name[1:])
                    name = names[start:names.index(b"/\n", start)].decode()
                directory = into / str(len(members))
                directory.mkdir()
                path = directory / name.removesuffix("/")
                path.write_bytes(body)
                members.append(path)
    return members


def smoke(goos, cc, archive, outdir, env):
    """Link the archive the way a consumer does and run it, so a broken or
    over-stripped artifact fails here rather than in a downstream build."""
    binary = outdir / ("smoke.exe" if goos == "windows" else "smoke")
    run([cc, str(ROOT / "tests/smoke.c"), str(archive), "-I", str(ROOT / "include"),
         "-o", str(binary), *SYSTEM_LIBS[goos]], env=env)
    run([str(binary)])
    binary.unlink()


if __name__ == "__main__":
    main()
