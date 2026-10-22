#!/usr/bin/env bash
# Test VM for neslina: UEFI firmware, the boot medium as a USB stick, an empty
# state disk, serial console on the terminal.
#
#   scripts/vm.sh stick <file.efi> [file...]
#                                    write a GPT stick image with the EFI file as EFI/BOOT/BOOTX64.EFI
#                                    and every file (the EFI file too) in neslina/
#   scripts/vm.sh stick-slot <slot> [slot...]
#                                    the same, for slots made by `neslina build`;
#                                    the first is the firmware's fallback (the rescue slot)
#   scripts/vm.sh bundle [dir]       copy an install bundle (`neslina bundle`; default output/bundle)
#                                    into neslina/install/ on the stick
#   scripts/vm.sh want <slot>        ask the host to try a slot, through neslina/boot.conf
#   scripts/vm.sh put <file>         copy a file (setup.toml, for one) into neslina/ on the stick
#   scripts/vm.sh wipe-state         start again from an empty state disk
#   scripts/vm.sh run                boot the VM (Ctrl-a x quits, Ctrl-a c opens the QEMU monitor)
#   scripts/vm.sh reset-nvram        start again from empty UEFI variables
#
# Environment:
#   NESLINA_VM_DIR   where the disk images and UEFI variables live (default: output/vm)
#   VM_CPUS          vCPUs (default: 8)
#   VM_MEM           memory (default: 16G)
#   VM_STATE_SIZE    size of the state disk when it is first created (default: 64G)
#   VM_HTTPS_PORT    host port forwarded to the guest's 443 (default: 8443)
#   VM_TAP           a tap device on your LAN bridge, owned by you: the VM then gets its
#                    address from your DHCP server and answers mDNS there. Without it,
#                    QEMU's user networking (DHCP from QEMU, no multicast, port forward only)
#   OVMF_CODE        firmware code image (default: searched for)
#   OVMF_VARS        firmware variables template (default: searched for)
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
dir=${NESLINA_VM_DIR:-$repo/output/vm}
cpus=${VM_CPUS:-8}
mem=${VM_MEM:-16G}
state_size=${VM_STATE_SIZE:-64G}
https_port=${VM_HTTPS_PORT:-8443}

stick=$dir/stick.img
state=$dir/state.img
vars=$dir/OVMF_VARS.fd

die() {
	echo "vm.sh: $*" >&2
	exit 1
}

find_first() {
	local f
	for f in "$@"; do
		[[ -f $f ]] && { echo "$f"; return 0; }
	done
	return 1
}

find_ovmf() {
	OVMF_CODE=${OVMF_CODE:-$(find_first \
		/usr/share/edk2/x64/OVMF_CODE.4m.fd \
		/usr/share/OVMF/OVMF_CODE_4M.fd \
		/usr/share/edk2/ovmf/OVMF_CODE.fd \
		/usr/share/OVMF/OVMF_CODE.fd)} || die "no OVMF code image found; set OVMF_CODE"
	OVMF_VARS=${OVMF_VARS:-$(find_first \
		/usr/share/edk2/x64/OVMF_VARS.4m.fd \
		/usr/share/OVMF/OVMF_VARS_4M.fd \
		/usr/share/edk2/ovmf/OVMF_VARS.fd \
		/usr/share/OVMF/OVMF_VARS.fd)} || die "no OVMF variables template found; set OVMF_VARS"
}

# A real stick is GPT with one ESP, so the image is too. mtools writes the FAT
# filesystem at the partition's offset, so no loop device or root is needed.
make_stick() {
	local efi=$1 f
	for f in "$@"; do
		[[ -f $f ]] || die "$f does not exist"
	done
	mkdir -p "$dir"
	rm -f "$stick"
	# Big enough for an install bundle; the image is sparse.
	truncate -s 4G "$stick"
	sfdisk --quiet "$stick" <<-EOF
		label: gpt
		start=2048, size=4000MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=NESLINA
	EOF
	local img="$stick@@1M"
	mformat -i "$img" -F -v NESLINA -T $((4000 * 2048)) ::
	mmd -i "$img" ::/EFI ::/EFI/BOOT ::/neslina
	mcopy -i "$img" "$efi" ::/EFI/BOOT/BOOTX64.EFI
	mcopy -i "$img" "$@" ::/neslina/
	echo "wrote $stick: $efi as EFI/BOOT/BOOTX64.EFI, and $# file(s) in neslina/"
}

run() {
	find_ovmf
	[[ -f $stick ]] || die "no stick image; run: scripts/vm.sh stick <file.efi>"
	[[ -f $vars ]] || cp "$OVMF_VARS" "$vars"
	[[ -f $state ]] || truncate -s "$state_size" "$state"
	[[ -r /dev/kvm ]] || die "/dev/kvm is not accessible"

	local netdev=(-netdev "user,id=net0,hostname=neslina-test,hostfwd=tcp:127.0.0.1:$https_port-:443")
	if [[ -n ${VM_TAP:-} ]]; then
		netdev=(-netdev "tap,id=net0,ifname=$VM_TAP,script=no,downscript=no")
	fi

	# -cpu host: native builds inside the VM see the real CPU, and nesbox gets VMX/SVM (nested KVM).
	# The stick goes on xHCI so firmware and kernel see a USB device, as on real hardware.
	# i6300esb with watchdog=reset is what makes a hung trial slot fall back.
	exec qemu-system-x86_64 \
		-name neslina \
		-machine q35,accel=kvm \
		-cpu host \
		-smp "$cpus" \
		-m "$mem" \
		-drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
		-drive if=pflash,format=raw,file="$vars" \
		-device qemu-xhci,id=xhci \
		-drive if=none,id=stick,format=raw,file="$stick" \
		-device usb-storage,bus=xhci.0,drive=stick,removable=on,bootindex=0 \
		-drive if=none,id=state,format=raw,file="$state",cache=none,aio=io_uring,discard=unmap \
		-device virtio-blk-pci,drive=state,serial=neslina-state \
		"${netdev[@]}" \
		-device virtio-net-pci,netdev=net0 \
		-device i6300esb \
		-action watchdog=reset \
		-nographic \
		-serial mon:stdio
}

case ${1:-} in
stick)
	[[ $# -ge 2 ]] || die "usage: scripts/vm.sh stick <file.efi> [file...]"
	shift
	make_stick "$@"
	;;
stick-slot)
	[[ $# -ge 2 ]] || die "usage: scripts/vm.sh stick-slot <slot> [slot...]"
	shift
	files=()
	for s in "$@"; do
		files+=("$repo/output/image/$s/$s.efi" "$repo/output/image/$s/$s.erofs")
	done
	make_stick "${files[@]}"
	;;
want)
	[[ $# -eq 2 ]] || die "usage: scripts/vm.sh want <slot>"
	[[ -f $stick ]] || die "no stick image; run: scripts/vm.sh stick-slot <slot...>"
	printf 'want=%s\n' "$2" >"$dir/boot.conf"
	mcopy -o -i "$stick@@1M" "$dir/boot.conf" ::/neslina/boot.conf
	echo "wrote neslina/boot.conf: want=$2"
	;;
bundle)
	[[ -f $stick ]] || die "no stick image; run: scripts/vm.sh stick-slot <slot...>"
	from=${2:-$repo/output/bundle}
	[[ -f $from/sysroot.erofs && -f $from/source.erofs ]] || die "no bundle in $from; run: neslina bundle"
	mmd -i "$stick@@1M" ::/neslina/install 2>/dev/null || true
	mcopy -o -i "$stick@@1M" "$from/sysroot.erofs" "$from/source.erofs" ::/neslina/install/
	echo "copied the bundle from $from to neslina/install/"
	;;
put)
	[[ $# -eq 2 && -f $2 ]] || die "usage: scripts/vm.sh put <file>"
	[[ -f $stick ]] || die "no stick image; run: scripts/vm.sh stick-slot <slot...>"
	mcopy -o -i "$stick@@1M" "$2" "::/neslina/$(basename "$2")"
	echo "copied $2 to neslina/$(basename "$2")"
	;;
wipe-state)
	rm -f "$state"
	echo "removed $state; the next run starts with an empty state disk"
	;;
run) run ;;
reset-nvram)
	find_ovmf
	mkdir -p "$dir"
	cp "$OVMF_VARS" "$vars"
	echo "reset $vars"
	;;
*) die "usage: scripts/vm.sh stick <file.efi> [file...] | stick-slot <slot...> | want <slot> | bundle [dir] | put <file> | wipe-state | run | reset-nvram" ;;
esac
