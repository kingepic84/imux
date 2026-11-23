#!/bin/bash
# Builds the release archives for the four supported systems, on any Linux machine that has
# cargo-zigbuild, Zig and the four Rust targets (the CI image has them).
#
#   scripts/release.sh [DIST_DIR]
#
# For each target it writes DIST_DIR/imux-<target>.tar.gz, which holds the binary `imux` alone, and
# DIST_DIR/imux-<target>.tar.gz.sha256 (the checksum, a space, the file name). install.sh and the
# Homebrew formula download exactly these.
#
# The macOS binaries are linked by Zig without an Apple SDK, which works because imux uses only
# libc. They target macOS 11 and newer; the arm64 one carries the ad-hoc signature that Apple
# Silicon requires. Linux binaries need glibc 2.28 or newer (Debian 10, Ubuntu 18.10, RHEL 8).
set -eu
cd "$(dirname "$0")/.."
dist="${1:-dist}"
mkdir -p "$dist"
export MACOSX_DEPLOYMENT_TARGET=11.0

# target, then the Zig-flavored target to build it for
targets="
aarch64-unknown-linux-gnu aarch64-unknown-linux-gnu.2.28
x86_64-unknown-linux-gnu x86_64-unknown-linux-gnu.2.28
aarch64-apple-darwin aarch64-apple-darwin
x86_64-apple-darwin x86_64-apple-darwin
"

echo "$targets" | while read -r target zigtarget; do
  [ -n "$target" ] || continue
  echo "== $target"
  cargo zigbuild --release --locked --target "$zigtarget"
  bin="target/$target/release/imux"
  stage="$(mktemp -d)"
  install -m 755 "$bin" "$stage/imux"
  archive="$dist/imux-$target.tar.gz"
  # Same bytes for the same input: sorted, no owner, no time stamps.
  tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner -C "$stage" -cf - imux | gzip -9n > "$archive"
  rm -rf "$stage"
  (cd "$dist" && { sha256sum "imux-$target.tar.gz" 2>/dev/null || shasum -a 256 "imux-$target.tar.gz"; } > "imux-$target.tar.gz.sha256")
done
ls -l "$dist"
