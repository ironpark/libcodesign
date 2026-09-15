# libcodesign

Prebuilt static libraries that expose [apple-codesign](https://github.com/indygreg/apple-platform-rs)
(rcodesign) through a small C ABI, so non-macOS programs can sign, notarize and
staple Apple bundles without shelling out to a CLI.

Built for [zapp](https://github.com/ironpark/zapp), which links the Windows and
Linux archives through cgo.

## Downloads

Every tag publishes one archive per target on the
[releases page](https://github.com/ironpark/libcodesign/releases), plus a
`SHA256SUMS` file covering all of them.

| Target | Archive | Toolchain it links with |
| --- | --- | --- |
| `linux_amd64` | `libcodesign_<version>_linux_amd64.tar.gz` | gcc/clang, glibc 2.35+ |
| `linux_arm64` | `libcodesign_<version>_linux_arm64.tar.gz` | gcc/clang, glibc 2.35+ |
| `windows_amd64` | `libcodesign_<version>_windows_amd64.tar.gz` | [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) (UCRT) |
| `windows_arm64` | `libcodesign_<version>_windows_arm64.tar.gz` | [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) (UCRT) |

macOS is intentionally absent: Apple's own tooling is available there.

Each archive contains:

```
lib/libzapp_rcodesign.a
include/zapp_rcodesign.h
LICENSE NOTICE THIRD_PARTY_LICENSES.html Cargo.lock README.md
```

```sh
version=v0.1.0
target=linux_amd64
base=https://github.com/ironpark/libcodesign/releases/download/$version
curl -fsSLO $base/libcodesign_${version#v}_$target.tar.gz
curl -fsSL $base/SHA256SUMS | grep "_$target.tar.gz$" | sha256sum -c -
tar xzf libcodesign_${version#v}_$target.tar.gz -C third_party/libcodesign
```

## API

Two entry points, declared in `include/zapp_rcodesign.h`:

```c
char *zapp_rcodesign_run(const char *request);  /* NULL on success */
void zapp_rcodesign_free(char *error);
```

`request` is a NUL-terminated UTF-8 JSON document. The call is synchronous, never
prompts on stdin and never spawns a subprocess; a panic is caught and returned as
an error string rather than unwinding into the caller.

```json
{
  "operation": "sign",
  "path": "/path/to/Target.app",
  "options": {
    "p12_file": "", "p12_password": "", "p12_password_file": "",
    "pem_file": "", "api_key_file": ""
  }
}
```

| `operation` | Required option | Effect |
| --- | --- | --- |
| `sign` | `p12_file` or `pem_file` | Signs in place with the hardened runtime and an Apple timestamp |
| `submit` | `api_key_file` | Submits to notarization and waits up to 10 minutes |
| `staple` | — | Staples the notarization ticket |

## Linking

Link the archive first, then the system libraries it depends on:

```sh
# linux
cc main.c lib/libzapp_rcodesign.a -Iinclude -ldl -lpthread -lm
# windows (llvm-mingw)
clang main.c lib/libzapp_rcodesign.a -Iinclude -static -lws2_32 -luserenv -lbcrypt \
  -lntdll -lcrypt32 -lncrypt -lsecur32 -liphlpapi -lole32 -loleaut32 -lruntimeobject
```

From Go, with cgo:

```go
/*
#cgo linux LDFLAGS: ${SRCDIR}/lib/libzapp_rcodesign.a -ldl -lpthread -lm
#cgo windows LDFLAGS: ${SRCDIR}/lib/libzapp_rcodesign.a -static -lws2_32 -luserenv -lbcrypt -lntdll -lcrypt32 -lncrypt -lsecur32 -liphlpapi -lole32 -loleaut32 -lruntimeobject
#include <stdlib.h>
#include "zapp_rcodesign.h"
*/
import "C"
```

`tests/smoke.c` is the same link, built and run in CI for every target, so a
published archive is one that has already linked and executed.

## Building it yourself

Rust 1.98.0 and Python 3 are enough on Linux; Windows also needs llvm-mingw on
`PATH`. Builds are native per target — the release matrix runs each one on its
own runner rather than cross-compiling.

```sh
python scripts/build.py linux_amd64 --test --strip --smoke
python scripts/package.py linux_amd64   # needs THIRD_PARTY_LICENSES.html
```

`--strip` removes debug sections (`strip --strip-debug`), which is the difference
between a ~900 MB archive and a shippable one; the symbol table and code are
untouched. `--smoke` links and runs `tests/smoke.c` against the stripped result.
The notices file comes from `cargo about generate about.hbs -o THIRD_PARTY_LICENSES.html`
(`cargo install cargo-about --locked --version 0.8.4`).

## Releasing

Push a `v*` tag. `.github/workflows/release.yaml` builds all four targets, packages
them, and publishes the archives with a combined `SHA256SUMS`. `workflow_dispatch`
can rebuild and publish an existing tag.

## Licensing

The binding is MIT (`LICENSE`). The archive statically links apple-codesign
(MPL-2.0) and other Rust crates — see `NOTICE` and `THIRD_PARTY_LICENSES.html`,
both of which ship inside every release archive and must be redistributed with it.
