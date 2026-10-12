# neslina

**A small Linux host OS for running Nestri boxes, built on the machine it runs on.**

Write it to a USB stick and boot a machine with a GPU from it. The host shows up
on your LAN with a setup page. There you pick its disks, and it builds itself
for that exact CPU and GPU, then reboots as a Nestri host.

```
write USB  →  boot headless  →  setup page on LAN  →  native build  →  host ready
```

## Why

A cloud-gaming host should give the guests as much of the machine as it can.
So neslina is compiled on the host, for the host: the kernel with
`CONFIG_X86_NATIVE_CPU`, and Mesa, virglrenderer and the rest with
`-march=native`. We ship pinned sources and recipes, not packages built for
the lowest common CPU.

## What it is

- Runs from RAM. It boots from the USB stick, or from a small partition on an
  internal disk, and copies a compressed root image into memory.
- Headless. No display, no keyboard, no shell. It is managed from another
  device through a web UI.
- Its own userspace: a Rust init, the `neslina` tool, and the few C components
  the workload needs. No base distro and no systemd.
- A/B slots with automatic rollback, plus a generic rescue slot. There is no
  bootloader: the firmware starts each slot's kernel directly.

## What it is not

- A general-purpose distro. It is built for Nestri's own deployments; anyone who
  wants a dedicated self-hosted Nestri box is welcome to use it.
- A gaming OS. Gaming is one workload on it.

## Requirements

x86_64 with x86-64-v3 (AVX2), UEFI, a GPU from AMD or NVIDIA, one internal disk,
and DHCP on the network.

## Status

Milestone 1: a generic build boots in QEMU from a USB stick image, with no
bootloader. It runs from RAM and gets a DHCP address. Install, native builds and
the GPU stack are next. The plan is in [`docs/DESIGN.md`](docs/DESIGN.md).

## Trying it

`scripts/build-image.sh` builds a slot: the kernel as one EFI file and the root
image it boots. `scripts/vm.sh` runs a QEMU VM with UEFI firmware, the slot on a
USB stick image, an empty state disk and the serial console on your terminal.

They need clang and lld, Rust, erofs-utils, QEMU, OVMF, mtools and util-linux.
The first build clones and patches the kernel into `output/linux`.

```bash
scripts/build-image.sh
```
```bash
scripts/vm.sh stick-slot rescue
```
```bash
scripts/vm.sh run
```

In the VM, `Ctrl-a c` opens the QEMU monitor; `sendkey ctrl-alt-delete` there
reboots the host cleanly, and `Ctrl-a x` quits.

## License

Apache-2.0, the same as the main Nestri repository. See [`LICENSE`](LICENSE).
