# neslina design

Status: draft, 2026-10-12. Replaces the 2026-10-03 draft. Nothing here is built.

This revision covers the install phase and a booted minimal system. How a host
is managed afterwards, and how Nestri software lands on it, comes later (§11).

## 1. Goal

A Linux host that gives as much of the machine as possible to the guests it
runs. It is built for Nestri's own deployments; anyone who wants a dedicated
self-hosted Nestri box can use it, with the wiki as their guide. It is not a
general-purpose distro, and edge cases outside that use are not supported.

What that means in practice:

- **Built on the host, for the host.** The performance-relevant parts are
  compiled on the machine they run on, with `-march=native` and the kernel's
  `CONFIG_X86_NATIVE_CPU`. We ship pinned sources and recipes, not packages.
- **Small, and runs from RAM.** The root image is loaded into RAM at boot, so it
  costs as little memory as we can make it.
- **Headless.** No display, no keyboard, no shell. Everything is done from
  another device through the web UI.
- **Our own userspace.** No base distro and no systemd. A Rust init, a Rust
  `neslina` tool, and the few C components the workload needs.

Security comes from nesbox: seccomp, the jailer and the microVM. neslina does
not add Secure Boot, TPM binding or a verified root. Each of those costs setup
steps that cannot be automated on a headless machine.

## 2. Hardware

- x86_64 with x86-64-v3 (AVX2) or newer. The guests are built for v3, and many
  games need AVX2 anyway.
- UEFI boot.
- A GPU from AMD or NVIDIA (Intel where nesbox supports it).
- One internal disk for state, which also holds the build tree (§5).
- Ethernet with a DHCP server on the network.

## 3. Disks and boot media

The owner assigns disks in the setup UI:

| Role | Required | Filesystem | Holds |
|---|---|---|---|
| Boot | yes | FAT32 (ESP) | Kernel EFI files and root images, per slot |
| State | yes | XFS | Config, logs, build sysroot, sources, build tree, Nestri data |
| Games | no | XFS | Game images. Falls back to the state disk. |

The boot role is either:

- **USB mode:** the stick setup was booted from. The root image is copied into
  RAM at boot and the stick is not touched again until an update. This frees
  every internal disk for state and games.
- **Internal mode:** a small ESP partition on the state disk. The root image is
  mounted in place; the page cache keeps it in RAM anyway.

The boot medium only needs FAT32, because the UEFI firmware reads it and
nothing else is stored there.

Layout of the boot medium:

```
EFI/BOOT/BOOTX64.EFI      rescue kernel (firmware's fallback path)
neslina/rescue.efi        generic kernel, built by CI
neslina/rescue.erofs      generic root image
neslina/a.efi  a.erofs    native slot A
neslina/b.efi  b.erofs    native slot B
neslina/setup.toml        setup password and seed config (written by the image writer)
```

## 4. Boot

No bootloader. Each slot's kernel is one EFI executable that the firmware
starts directly:

- **Kernel:** a bzImage with `CONFIG_EFI_STUB`.
- **Built-in command line:** `CONFIG_CMDLINE`, with `CONFIG_CMDLINE_OVERRIDE`
  so stale firmware load options cannot change it.
- **Built-in initramfs:** `CONFIG_INITRAMFS_SOURCE`. It holds:
  - stage 1;
  - the firmware that built-in drivers need before the root image is mounted
    (the GPU's, for example);
  - the hash of the root image this kernel belongs to.
- **Microcode:** built into the kernel with `CONFIG_EXTRA_FIRMWARE`. A native
  build knows the exact CPU, so only its microcode is included.

**Stage 1** is our own small static binary. In order:

1. Mounts the boot medium.
2. Finds the slot's `.erofs` file.
3. Checks its hash. This catches a corrupt USB stick, not an attacker.
4. Copies the image into RAM if the slot is in USB mode.
5. Mounts it. erofs mounts a plain file directly, so no loop device is needed.
6. Switches root and executes init.

The image stays compressed in RAM, because erofs decompresses on read.

### Slots, updates and rollback

Slots are UEFI boot entries (`Boot####` variables), which neslina writes itself
through efivarfs. No `efibootmgr` is needed.

- **Trying a new slot:** set `BootNext` to it and reboot. `BootNext` is used
  once, and the next reboot goes back to `BootOrder`.
- **Marking it good:** once init has reached its health point, it moves the
  slot to the front of `BootOrder`. A slot that panics (`panic=10`) or trips the
  watchdog reboots into the previous slot without anyone touching the machine.
- **Rescue:** the generic image from CI stays installed as the fallback
  `BOOTX64.EFI`. Some firmware loses NVRAM entries, especially for USB devices;
  the fallback path still boots something. Rescue is also the path for a
  hardware swap: boot rescue, rebuild.
- **Watchdog:** a hardware watchdog driver is built in, and init arms it as
  early as it can. A hang before init starts, with no watchdog armed by the
  firmware, still needs a manual reset. That case is accepted.

**To test:** how real firmware treats NVRAM entries pointing at USB devices.
Booting without a bootloader is the plan, and our side carries whatever that
needs. Limine stays a fallback only if hardware forces it, and it does not run
on all older hardware.

## 5. Install phase

1. **Write the image.** The image writer (or the download page) puts the
   generic install image on a USB stick. It generates a random setup password
   into `neslina/setup.toml`, which the owner can read or change on any PC.
2. **Boot it headless.** The owner may have to pick USB in the firmware's
   boot menu once.
3. **Find the host.** It takes an address over DHCP, sends its hostname
   (`neslina-<last 4 of MAC>`) to the DHCP server, and advertises
   `<hostname>.local` over mDNS.
4. **Log in to setup.** The setup UI is served on the LAN. The first login
   uses the password from the stick and forces a new one.
5. **Review.** Setup shows what it detected (CPU, GPU, disks, network) and
   what it proposes. The owner assigns disk roles and confirms which disks it
   may wipe.
6. **Partition and format.** Setup partitions and formats the assigned disks.
7. **Unpack.** It copies the prebuilt sysroot and the pinned sources from the
   install medium onto the state disk. Install needs no network beyond DHCP.
8. **Native build** (§6), with progress and logs in the setup UI.
9. **Install and reboot.** The native image is written as slot A and tried
   with `BootNext`. Setup is finished when slot A marks itself good. If it
   fails, the host comes back on rescue and the setup UI shows why.

There is no static IP and no local network config. DHCP only.

The setup UI is HTTPS only, with a self-signed certificate generated on first
boot and kept on the state disk afterwards. The browser warns once, as it does
for Unraid or Cockpit.

## 6. Building

Building is split by what gains from native code:

- **Prebuilt (by CI, shipped as one `sysroot.erofs`):**
  - LLVM, clang and lld 23.1; mold 3.0;
  - rustc and cargo;
  - glibc;
  - build tools: Python and meson, ninja, cmake, Perl, pkgconf, bison, flex
    and so on.

  The sysroot is mounted only while building. It is not part of the running
  OS.
- **Built on the host:** everything that ends up in the root image. The ones
  that matter are the kernel, Mesa, virglrenderer and the NVIDIA kernel module.
  The rest is built natively because it costs little, not because it is
  measured to help.

The first sysroot is built in an ordinary distro container in CI. That
container exists only at build time and nothing from it ships. Once neslina can
build its own sysroot, it takes over.

### Toolchain rules

- The kernel and userspace use the same LLVM.

- Kernel and kernel modules (including NVIDIA's): `LLVM=1`, linked with
  `ld.lld`. Clang LTO requires lld (`HAS_LTO_CLANG` depends on `LD_IS_LLD`),
  and the kernel's linker scripts are beyond what mold handles.
- Everything else: clang and mold, with ThinLTO where the project builds with
  it.
- AutoFDO and Propeller need clang only. Profiles come later (§11).

### Recipes

One TOML file per component in `recipes/`:

- source URL and hash;
- patches;
- build system (meson, cmake, autotools, cargo, kbuild);
- flags;
- the files it installs into the image.

They are data, so adding a component does not mean rebuilding `neslina`.
Patches worth taking from CachyOS and others are carried in the recipe,
each with a note on why.

Builds run without network access (sources are already on disk and hashed),
as an unprivileged user in a namespace.

CI uses the same builder and recipes, with x86-64-v3 as the target, to make the
generic image and rescue slot. One tool, one recipe format, and only the target
flags differ.

## 7. Kernel

- **Base:** CachyOS `linux`. The current pin is `cachyos-7.2.9-1`.
- **Config:** one curated headless config plus `CONFIG_X86_NATIVE_CPU`.
  Everything the host needs is built in, so there is no module loading except
  NVIDIA's. Patches live in `kernel/patches/` (see `kernel/README.md`).
- **`CONFIG_RUST` off.** `X86_NATIVE_CPU` depends on `!RUST || RUSTC_HAS_APXF`,
  and the host needs none of the Rust drivers. The config sets it explicitly
  rather than inheriting CachyOS's choice.

What it must provide, from what nesbox uses:

- **KVM:** `KVM`, `KVM_AMD` and `KVM_INTEL`.
- **Event and I/O plumbing:** `EVENTFD`, `VHOST_NET`, `VHOST_VSOCK`, and `TUN`
  with vnet headers.
- **Memory:** `MEMFD_CREATE`, shmem with `TRANSPARENT_HUGEPAGE`,
  `HUGETLB_PAGE`.
- **Block I/O:** `IO_URING`.
- **Sandboxing:** `SECCOMP_FILTER`.
- **Namespaces and filesystems:** mount, user and network namespaces;
  `OVERLAY_FS`, `TMPFS`.
- **cgroups:** cgroup v2 with cpu, cpuset, memory, io, pids, hugetlb and
  `CGROUP_DMEM`.
- **GPU:** amdgpu, i915 and xe, with per-client DRM fdinfo.
- **Networking:** nftables (with NAT) and bridging.
- **Boot path:** `EFI_STUB`, `EROFS_FS` with `EROFS_FS_BACKED_BY_FILE`, `VFAT`,
  a hardware watchdog.
- **Default command line:**
  - `gpu_sched.sched_policy=2`
  - `transparent_hugepage_shmem=advise`
  - `panic=10`
- **No swap.**

## 8. Userspace

There is no shell, no package manager and no interpreter in the running
image.

| Job | Done by |
|---|---|
| PID 1: mounts, supervision, reaping, ordered shutdown, cgroup tree | `init` |
| Device permissions (kvm and render nodes 0666) and disk hotplug events | `init`, listening for uevents. devtmpfs creates the nodes and the kernel loads firmware itself, so there is no udev. |
| NVIDIA module | `init`, with one `finit_module` call |
| DHCP, links, bridges and taps | rtnetlink and a DHCP client, in Rust |
| DNS | `/etc/resolv.conf` written from DHCP |
| Time | An SNTP client |
| Logs | A RAM ring buffer plus `/dev/kmsg`, flushed to the state disk |
| Setup and later management | The `neslina` web UI |

`init` takes its supervision model from `nesinit`: it starts services and
keeps them alive. It is its own binary, with as few dependencies as possible.
A panic in PID 1 is a kernel panic, so the web server, the builder, and HTTP and
TLS stay out of it.

C components in the image, to begin with:

- glibc;
- xfsprogs and dosfstools, for setup.

The GPU stack (Mesa, virglrenderer, NVIDIA userspace) and virtiofsd come in at
milestone 3.

## 9. Repository layout

```
crates/
  common/   shared config, logging and IPC
  init/     PID 1 (separate binary, minimal deps)
  stage1/   initramfs: find, verify and mount the root image
  build/    recipes, native builds, image assembly
  setup/    install-phase web UI, disk detection and formatting
  cli/      the `neslina` binary: `neslina system ...`, `neslina setup ...`
recipes/    one TOML per component
kernel/     curated config and patches
docs/
```

## 10. Milestones

1. **Boots in QEMU.** CI-style generic build of the kernel, stage 1 and init.
   `scripts/vm.sh` boots it with OVMF from a USB stick image, with no
   bootloader. It gets a DHCP address and logs to serial.
2. **Install in QEMU.** Setup UI, disk assignment, unpacking the sysroot, a
   native kernel build, slot A tried with `BootNext` and marked good. Rollback
   proven by installing a slot that fails on purpose and checking that the
   host comes back on the old one.
3. **GPU stack.** Mesa, virglrenderer and the NVIDIA module built natively; a
   GPU visible to nesbox on a lab machine.
4. **Real hardware.** USB mode and internal mode on the lab machines, and how
   firmware handles NVRAM entries (§4).

## 11. Later

These come after the install phase works, and are not designed yet:

- Managing a host after setup, either through Nestri's web UI in the cloud or a
  local one on the LAN.
- Delivering prebuilt Nestri binaries (neslet and others) with no sources on
  the host.
- Updates: new pinned recipes, rebuild into the inactive slot, try it with
  `BootNext`.
- PGO, AutoFDO and Propeller: turn profiling on from the web UI, run a real
  workload, rebuild with the profile. AutoFDO and Propeller need hardware
  branch sampling (Intel LBR, AMD BRS/LbrExtV2).
- Live kernel update: Kexec HandOver (KHO) and the Live Update Orchestrator
  (LUO) are in 7.2 and can preserve memfds across a kexec, and nesbox keeps
  guest RAM in a memfd. GPU state does not survive, so for now kexec only saves
  the firmware's boot time; boxes still restart.
