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

In QEMU, a stick holding the rescue slot, an install bundle and a `setup.toml`
installs a host with nobody touching it. Rescue formats the state disk it was
told to, builds the host's own native slot from the bundle, and boots into it
with no bootloader; A/B slots try, promote and roll back. The setup UI and the
GPU stack are next. The plan is in [`docs/DESIGN.md`](docs/DESIGN.md).

## Trying it

Builds run inside a prebuilt toolchain sysroot, in a sandbox with no network.
Making the sysroot needs podman; building needs Rust; the VM needs QEMU, OVMF,
mtools and util-linux.

```bash
scripts/build-sysroot.sh
```
```bash
cargo build --release -p neslina
```
```bash
target/release/neslina fetch
```
```bash
target/release/neslina build --slot rescue
```
```bash
scripts/vm.sh stick-slot rescue
```
```bash
scripts/vm.sh run
```

`neslina build --cpu native` builds for the machine running it; the VM passes
the host CPU through, so a native slot built on the same machine boots there.

To try a slot, build it under another name (`--slot a`), put it on the stick
beside rescue (`scripts/vm.sh stick-slot rescue a`), and ask for it with
`scripts/vm.sh want a`. `neslina build --broken panic|hang` makes a slot that
fails, to watch rollback.

To watch a full install, give the stick an install bundle and disk
assignment, and start from an empty state disk:

```bash
target/release/neslina bundle
```
```bash
scripts/vm.sh bundle
```
```bash
printf '[disks]\nstate = { serial = "neslina-state", wipe = true }\n' > output/setup.toml
```
```bash
scripts/vm.sh put output/setup.toml
```

In the VM, `Ctrl-a c` opens the QEMU monitor; `sendkey ctrl-alt-delete` there
reboots the host cleanly, and `Ctrl-a x` quits.

## License

Apache-2.0, the same as the main Nestri repository. See [`LICENSE`](LICENSE).
