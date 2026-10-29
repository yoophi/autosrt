#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  *) echo 'Release packaging currently requires macOS Apple Silicon.' >&2; exit 1 ;;
esac
export MACOSX_DEPLOYMENT_TARGET=15.0
cargo build --release --locked --target "$target"
archive="autosrt-${version}-${target}"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
mkdir -p "dist" "$stage/$archive"
cp "target/$target/release/autosrt" README.md LICENSE "$stage/$archive/"
COPYFILE_DISABLE=1 tar -czf "dist/$archive.tar.gz" -C "$stage" "$archive"
(cd dist && shasum -a 256 "$archive.tar.gz" > SHA256SUMS)
printf 'Created dist/%s.tar.gz\n' "$archive"
