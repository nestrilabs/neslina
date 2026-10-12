#!/usr/bin/env bash
# Builds the toolchain sysroot with podman.
#
#   scripts/build-sysroot.sh [ARCHIVE_DATE]
#
# ARCHIVE_DATE is a day of the Arch Linux Archive, as YYYY/MM/DD.
#
# Output, in output/sysroot/:
#   sysroot.erofs   what a host mounts while building
#   root/           the same tree unpacked, for building on a dev machine
#                   without root (an unprivileged user cannot mount erofs)
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
date=${1:-2026/10/11}
out=$repo/output/sysroot
image=localhost/neslina-sysroot:${date//\//-}

podman build --build-arg ARCHIVE_DATE="$date" -t "$image" "$repo/sysroot"

mkdir -p "$out"
id=$(podman create "$image" /bin/true)
trap 'podman rm -f "$id" >/dev/null' EXIT
podman export "$id" -o "$out/sysroot.tar"

rm -f "$out/sysroot.erofs"
mkfs.erofs --quiet --tar=f -T 0 -z lz4hc,12 "$out/sysroot.erofs" "$out/sysroot.tar"
rm -rf "$out/root"
mkdir -p "$out/root"
# Device nodes cannot be created unprivileged, and the sandbox makes its own /dev.
tar -x -f "$out/sysroot.tar" -C "$out/root" --exclude='dev/*' --no-same-owner
rm "$out/sysroot.tar"
echo "$date" >"$out/ARCHIVE_DATE"

echo "sysroot from archive $date:"
echo "$out/sysroot.erofs ($(du -h "$out/sysroot.erofs" | cut -f1))"
echo "$out/root"
