#!/bin/sh
# Refreshes the prebuilt libghostty-vt in libghostty-vt/ from an official release:
#   crates/ghostty-sys/update-prebuilt.sh 1.0.0
# macOS arm64 only for now: the release's xcframework, checked against its
# SHA256SUMS, thinned to arm64 and stripped of debug info. Other targets need
# their library added under libghostty-vt/<target>/ (build.rs says which).
set -eu
version=${1:?usage: update-prebuilt.sh <version, such as 1.0.0>}
here=$(cd "$(dirname "$0")" && pwd)
dest="$here/libghostty-vt"
base="https://release.files.ghostty.org/libghostty-vt/$version"
zip="ghostty-vt-$version.xcframework.zip"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cd "$work"
curl -sfLO "$base/SHA256SUMS"
curl -sfLO "$base/$zip"
grep "  $zip\$" SHA256SUMS | shasum -a 256 -c -
unzip -q "$zip"
mac=ghostty-vt.xcframework/macos-arm64_x86_64

rm -rf "$dest/include" "$dest/aarch64-apple-darwin"
mkdir -p "$dest/include" "$dest/aarch64-apple-darwin"
cp -R "$mac/Headers/ghostty" "$dest/include/"
lipo "$mac/libghostty-vt.a" -thin arm64 -output "$dest/aarch64-apple-darwin/libghostty-vt.a"
strip -S "$dest/aarch64-apple-darwin/libghostty-vt.a"
echo "$version" > "$dest/VERSION"
echo "libghostty-vt $version: $(du -h "$dest/aarch64-apple-darwin/libghostty-vt.a" | cut -f1) for aarch64-apple-darwin"
