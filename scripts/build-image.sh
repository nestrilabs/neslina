#!/usr/bin/env bash
# Builds one slot: a root image and the kernel EFI file that boots it.
#
# A stand-in until `neslina build` and the recipes exist (docs/DESIGN.md §6).
# It uses the build machine's own clang, lld, Rust and erofs-utils.
#
#   scripts/build-image.sh [--cpu generic|native] [--slot NAME]
#
# --cpu generic (the default) targets x86-64-v3, as CI and the rescue slot do.
# --cpu native targets the machine running the build.
#
# Output, in output/image/<slot>/:
#   <slot>.efi     kernel with command line and stage 1 initramfs built in
#   <slot>.erofs   root image; its hash is built into the kernel
#
# Environment:
#   NESLINA_KERNEL_SRC   kernel tree (default: output/linux, cloned and patched if missing)
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
kernel_src=${NESLINA_KERNEL_SRC:-$repo/output/linux}
kernel_repo=https://github.com/CachyOS/linux.git
kernel_ref=cachyos-7.2.9-1
cpu=generic
slot=rescue

die() {
	echo "build-image.sh: $*" >&2
	exit 1
}

while [[ $# -gt 0 ]]; do
	case $1 in
	--cpu) cpu=${2:?}; shift 2 ;;
	--slot) slot=${2:?}; shift 2 ;;
	*) die "unknown argument $1" ;;
	esac
done
case $cpu in
generic) rust_cpu=x86-64-v3 ;;
native) rust_cpu=native ;;
*) die "--cpu is generic or native" ;;
esac
[[ $slot =~ ^[a-z0-9-]+$ ]] || die "slot names are lowercase letters, digits and dashes"

out=$repo/output/image/$slot
kout=$repo/output/kernel-$cpu
# The kernel log shows up on serial and on a screen, if one is attached.
# printk.devkmsg=on: our programs log through /dev/kmsg, which is otherwise rate limited.
cmdline="console=tty0 console=ttyS0,115200 panic=10 printk.devkmsg=on transparent_hugepage_shmem=advise neslina.slot=$slot"

prepare_kernel() {
	# A worktree has a .git file rather than a directory.
	[[ -e $kernel_src/.git ]] && return
	echo "cloning $kernel_ref into $kernel_src"
	git clone --branch "$kernel_ref" --depth 1 "$kernel_repo" "$kernel_src"
	git -C "$kernel_src" switch -c "neslina-$kernel_ref"
	git -C "$kernel_src" am "$repo"/kernel/patches/*.patch
}

build_rust() {
	echo "building stage 1, init and services for $rust_cpu"
	# Static, so stage 1 runs with no libraries at all and the root image needs
	# none until the C components arrive. --target keeps RUSTFLAGS off build scripts.
	CARGO_TARGET_DIR=$repo/target/image-$cpu \
		RUSTFLAGS="-C target-feature=+crt-static -C target-cpu=$rust_cpu" \
		cargo build --manifest-path "$repo/Cargo.toml" --release --locked \
		--target x86_64-unknown-linux-gnu \
		-p neslina-stage1 -p neslina-init -p neslina-net
	bin=$repo/target/image-$cpu/x86_64-unknown-linux-gnu/release
}

build_root() {
	echo "assembling the root image"
	local root=$out/root
	rm -rf "$root"
	mkdir -p "$root"/{sbin,etc,dev,proc,sys,run,tmp,var,usr/bin,usr/lib/neslina}
	install -m 0755 "$bin/neslina-init" "$root/sbin/init"
	install -m 0755 "$bin/neslina-net" "$root/usr/lib/neslina/neslina-net"
	ln -s ../run/neslina/resolv.conf "$root/etc/resolv.conf"
	cat >"$root/etc/os-release" <<-EOF
		NAME=neslina
		ID=neslina
		PRETTY_NAME="neslina"
		VERSION_ID=$(git -C "$repo" describe --always --dirty)
		HOME_URL=https://github.com/nestrilabs/neslina
	EOF

	# Fixed timestamps and root ownership, so the same input gives the same image.
	mkfs.erofs --quiet --all-root -T 0 -z lz4hc,12 "$out/$slot.erofs" "$root"
	sha256=$(sha256sum "$out/$slot.erofs" | cut -d' ' -f1)
	printf 'slot=%s\nsha256=%s\n' "$slot" "$sha256" >"$out/slot"
}

write_initramfs_list() {
	# gen_init_cpio's format: device nodes without needing root.
	cat >"$out/initramfs.list" <<-EOF
		dir /dev 0755 0 0
		nod /dev/console 0600 0 0 c 5 1
		dir /proc 0755 0 0
		dir /sys 0755 0 0
		dir /medium 0755 0 0
		dir /newroot 0755 0 0
		dir /ram 0700 0 0
		dir /neslina 0755 0 0
		file /init $bin/neslina-stage1 0755 0 0
		file /neslina/slot $out/slot 0644 0 0
	EOF
}

# Every line of every fragment must survive olddefconfig. A "not set" line
# passes if the option is off or no longer offered.
verify_config() {
	local config=$kout/.config bad=0 line sym
	while IFS= read -r line; do
		if [[ $line =~ ^(CONFIG_[A-Za-z0-9_]+)= ]]; then
			grep -qxF -- "$line" "$config" || { echo "kernel config: wanted $line, got: $(grep -E "^(# )?${BASH_REMATCH[1]}[= ]" "$config" || echo missing)" >&2; bad=1; }
		elif [[ $line =~ ^#\ (CONFIG_[A-Za-z0-9_]+)\ is\ not\ set ]]; then
			sym=${BASH_REMATCH[1]}
			! grep -qE "^$sym=" "$config" || { echo "kernel config: wanted $sym off, got: $(grep -E "^$sym=" "$config")" >&2; bad=1; }
		fi
	done < <(cat "$@")
	[[ $bad -eq 0 ]] || die "kernel config does not match the fragments"
}

build_kernel() {
	echo "configuring the kernel ($cpu)"
	mkdir -p "$kout"
	printf 'CONFIG_CMDLINE="%s"\nCONFIG_INITRAMFS_SOURCE="%s"\n' "$cmdline" "$out/initramfs.list" >"$out/slot.config"
	local frags=("$repo/kernel/config/neslina.config" "$repo/kernel/config/$cpu.config" "$out/slot.config")

	make -s -C "$kernel_src" O="$kout" LLVM=1 x86_64_defconfig
	"$kernel_src/scripts/kconfig/merge_config.sh" -m -O "$kout" "$kout/.config" "${frags[@]}" >/dev/null
	make -s -C "$kernel_src" O="$kout" LLVM=1 olddefconfig
	verify_config "${frags[@]}"

	echo "building the kernel"
	make -s -C "$kernel_src" O="$kout" LLVM=1 -j"$(nproc)" bzImage
	cp "$kout/arch/x86/boot/bzImage" "$out/$slot.efi"
}

mkdir -p "$out"
prepare_kernel
build_rust
build_root
write_initramfs_list
build_kernel

echo "built slot $slot ($cpu):"
echo "$out/$slot.efi"
echo "$out/$slot.erofs ($(du -h "$out/$slot.erofs" | cut -f1), sha256 $sha256)"
