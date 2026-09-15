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
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIB = "libzapp_rcodesign.a"
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
        # --strip-debug only removes .debug_* sections: the symbol table the
        # linker resolves against, and the code itself, are left untouched.
        strip = env.get("STRIP") or ("llvm-strip" if goos == "windows" else "strip")
        run([strip, "--strip-debug", str(archive)])
    if args.smoke:
        smoke(goos, cc, archive, outdir, env)
    print(f"{archive} ({archive.stat().st_size / 1e6:.0f} MB)")


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
