#!/usr/bin/env bash
# Test VM for neslina: UEFI firmware, the boot medium as a USB stick, an empty
# state disk, serial console on the terminal.
#
#   scripts/vm.sh stick <file.efi>   write a GPT stick image with the file as EFI/BOOT/BOOTX64.EFI
#   scripts/vm.sh run                boot the VM (Ctrl-a x quits, Ctrl-a c opens the QEMU monitor)
#   scripts/vm.sh reset-nvram        start again from empty UEFI variables
#
# Environment:
#   NESLINA_VM_DIR   where the disk images and UEFI variables live (default: output/vm)
#   VM_CPUS          vCPUs (default: 8)
#   VM_MEM           memory (default: 16G)
#   VM_STATE_SIZE    size of the state disk when it is first created (default: 64G)
#   VM_HTTPS_PORT    host port forwarded to the guest's 443 (default: 8443)
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
	local efi=$1
	[[ -f $efi ]] || die "$efi does not exist"
	mkdir -p "$dir"
	rm -f "$stick"
	truncate -s 1G "$stick"
	sfdisk --quiet "$stick" <<-EOF
		label: gpt
		start=2048, size=1000MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=NESLINA
	EOF
	local img="$stick@@1M"
	mformat -i "$img" -F -v NESLINA -T $((1000 * 2048)) ::
	mmd -i "$img" ::/EFI ::/EFI/BOOT ::/neslina
	mcopy -i "$img" "$efi" ::/EFI/BOOT/BOOTX64.EFI
	echo "wrote $stick with $efi as EFI/BOOT/BOOTX64.EFI"
}

run() {
	find_ovmf
	[[ -f $stick ]] || die "no stick image; run: scripts/vm.sh stick <file.efi>"
	[[ -f $vars ]] || cp "$OVMF_VARS" "$vars"
	[[ -f $state ]] || truncate -s "$state_size" "$state"
	[[ -r /dev/kvm ]] || die "/dev/kvm is not accessible"

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
		-device virtio-blk-pci,drive=state \
		-netdev user,id=net0,hostname=neslina-test,hostfwd=tcp:127.0.0.1:"$https_port"-:443 \
		-device virtio-net-pci,netdev=net0 \
		-device i6300esb \
		-action watchdog=reset \
		-nographic \
		-serial mon:stdio
}

case ${1:-} in
stick)
	[[ $# -eq 2 ]] || die "usage: scripts/vm.sh stick <file.efi>"
	make_stick "$2"
	;;
run) run ;;
reset-nvram)
	find_ovmf
	mkdir -p "$dir"
	cp "$OVMF_VARS" "$vars"
	echo "reset $vars"
	;;
*) die "usage: scripts/vm.sh stick <file.efi> | run | reset-nvram" ;;
esac
