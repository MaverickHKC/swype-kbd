#!/usr/bin/env bash
# Assemble a self-contained tester tarball for swype-kbd.
#
#   ./packaging/make-release.sh
#
# Produces dist/swype-kbd-<version>-x86_64-linux.tar.gz containing the release
# binary, dictionary, launcher, README, and font license. The binary finds the
# dictionary sitting next to it, so the tarball needs no install step.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
arch="$(uname -m)"
name="swype-kbd-${version}-${arch}-linux"
stage="dist/$name"

echo ">> building release binary"
cargo build --release

echo ">> staging $stage"
rm -rf "$stage"
mkdir -p "$stage"
cp target/release/swype-kbd            "$stage/swype-kbd"
cp data/words_50k.txt                  "$stage/words_50k.txt"
cp packaging/run.sh                    "$stage/run.sh"
cp packaging/TESTER-README.md          "$stage/README.md"
cp crates/app/assets/DejaVuSans.LICENSE "$stage/DejaVuSans.LICENSE"
chmod +x "$stage/swype-kbd" "$stage/run.sh"

echo ">> recording build provenance"
{
    echo "swype-kbd $version"
    echo "git:   $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "glibc: built against $(ldd --version | head -1)"
    echo "needs: x86_64, glibc >= 2.34, wlroots compositor"
} > "$stage/BUILD-INFO.txt"

echo ">> packing tarball"
tar -C dist -czf "dist/${name}.tar.gz" "$name"
rm -rf "$stage"

echo ">> done: dist/${name}.tar.gz"
ls -lh "dist/${name}.tar.gz"
