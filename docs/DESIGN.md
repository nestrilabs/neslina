# neslina design

Status: draft, 2026-10-03. Every section ends with what is still open. Nothing
here is built.

## 1. Goal

A host operating system that does exactly one job: run Nestri boxes on a GPU,
safely, fast, and without anyone touching it.

Three properties, in priority order:

1. **Contain an escaped guest.** Assume nesbox, vDRM or virtio-nvgpu is
   eventually broken. What the attacker lands on must be small, read-only, and
   watched.
2. **Zero-touch.** Install once. Registration, updates, storage and recovery
   happen without the owner.
3. **Performance we control.** Kernel config, driver versions and scheduler
   tuning are ours, tested together, shipped together.

Non-goals: running anything other than Nestri workloads; a local UI; being a
distro other people build on.

## 2. Threat model

| Attacker | Starting point | What neslina must ensure |
|---|---|---|
| Malicious guest | Code execution inside a box | Escape into nesbox lands in a jailed, seccomp'd process on a read-only host; cannot persist, cannot reach other boxes' data, cannot reach the LAN by default |
| Host owner (BYO hardware) | Physical access | Cannot read other tenants' data at rest; tampering with the image is detected at boot and the host is refused by the control plane |
| Network attacker | Same LAN / path | No listening services except the stream ports; everything else is outbound to our servers over mutually authenticated TLS |
| Us (compromised build or control plane) | Signing key or API | Out of scope for v1; noted so we do not pretend otherwise |

Physical-access attackers on BYO hardware cannot be fully defeated. The aim is
detection (measured boot + attestation), not prevention.

**Open:** do BYO hosts ever run boxes for other people's teams? If yes, the
host-owner row becomes the hardest row and needs TPM attestation from day one.

## 3. Shape of the system

```
┌──────────────────────────────────────────────┐
│ ESP          UKI A │ UKI B   (signed)        │
├──────────────────────────────────────────────┤
│ root A  (erofs/squashfs, dm-verity, ro)       │
│ root B  (erofs/squashfs, dm-verity, ro)       │
├──────────────────────────────────────────────┤
│ /var    (ext4 or xfs, LUKS bound to TPM)      │
│   identity/  boxes/  cache/  logs/            │
└──────────────────────────────────────────────┘
```

- **Boot:** UEFI Secure Boot → unified kernel image (kernel + initrd + cmdline,
  one signed file) → dm-verity root hash in the cmdline → read-only root. Any
  modified byte fails verification.
- **Root:** compressed, read-only, identical on every host for a given version.
  `/etc` is generated at boot from the image plus the control plane's config;
  nothing is edited in place.
- **State:** one writable partition. The only things on it: host identity, box
  disks, image and shader caches, logs pending upload.
- **Init:** systemd, trimmed. It already gives us cgroups, namespaces, sysupdate,
  repart, measured boot hooks and credential handling; writing our own init
  buys nothing.

**Open:** build from what? Candidates are mkosi on top of an existing distro's
packages (fastest start), Yocto/Buildroot (smallest, most work), or forking
Bottlerocket's build (Rust tooling we already like, but its packaging is heavy).
Recommendation: mkosi for v1, revisit once the package list is stable.

## 4. Userspace

Everything that runs, and nothing else:

- `systemd` (init, journald, networkd, resolved, timesyncd)
- `neslet` — host agent; talks to the control plane, launches boxes, reports
  health
- `nesbox` — VMM, one jailed process per box (see nesbox `SECURITY.md`)
- GPU driver stack: kernel modules plus the userspace the host side needs
  (Mesa for AMD/Intel, the NVIDIA driver for virtio-nvgpu backends)
- a debug channel (section 7)

No shell in the image. No `sudo`, no package manager, no interpreters, no
compilers. A debug image variant that has them is fine as long as production
hosts refuse to boot it (different signing key).

Driver userspace is dlopened, so trimming it by `NEEDED` will break it. Build
the trimmed set by probing a real workload against a kept full copy.

## 5. Install and first boot

The owner's experience:

1. Download an image (or USB writer / PXE for fleets).
2. Boot it. Installer partitions the largest suitable disk, writes slot A, reboots.
3. Screen shows a short code and a QR. Owner enters it in the Nestri dashboard,
   which binds the host to their team.
4. Host enrolls: generates its key in the TPM, receives its identity, appears in
   the team. Done.

No keyboard input other than confirming which disk to wipe.

**Open:** fleet hosts (our inventory) should skip step 3 entirely via
pre-provisioned tokens or PXE. Same image, different first-boot path.

## 6. Updates

- Control plane announces a version; `neslet` downloads the signed image into
  the inactive slot, verifies, and schedules the switch when no box is running
  (or at a drain deadline the control plane sets).
- Reboot into the new slot with a boot counter. If `neslet` does not report
  healthy within N minutes, the bootloader falls back to the old slot and the
  host reports the failed version.
- Kernel, GPU driver, nesbox and neslet always move together as one version.
  No partial updates.

**Rollback must be proven by breaking it**: CI ships an image that deliberately
fails health checks and asserts the host comes back on the old slot. A rollback
path that has never fired is not known to work.

**Open:** live-patching for urgent kernel CVEs vs. just draining and rebooting.
Recommendation: reboot; the image is small and boxes are restartable.

## 7. Access and observability

There is no SSH daemon listening. Access is outbound only:

- `neslet` keeps a connection to our servers. An operator opens a session
  through `nessh`, which is authorized, time-limited and recorded server-side,
  and the host spawns a restricted session over that connection.
- Default sessions are read-only diagnostics (logs, GPU state, box list). A
  shell, if ever allowed, needs a second approval and is logged as an incident.
- Logs and metrics are pushed, not pulled.
- Host owners get the same diagnostics through the dashboard, not a console.

**Open:** what does a BYO owner do when the network is down and the host is
unreachable? Minimum: the local screen shows status and a recovery code;
"reinstall" from USB is always safe because state is on `/var`.

## 8. Storage

- Adding a disk: plug it in, the host reports it, the owner (or policy) assigns
  it in the dashboard. `neslet` formats and adds it to the box storage pool.
  Nobody edits fstab.
- Box disks live on the data pool, encrypted at rest with keys the host cannot
  export.

**Open:** pool layout (LVM thin, btrfs, ZFS is out for licensing). Recommendation:
LVM thin + XFS; simplest thing that gives fast snapshots for box images.

## 9. Networking

- DHCP by default, static via dashboard. No local config.
- Inbound: only the stream ports neslet opens per box. Everything else dropped
  by nftables generated from the image.
- Boxes get their own network namespace and cannot see the LAN unless the
  team enables it.
- Hosts on bad home uplinks are expected; nothing in boot or update may assume
  a good one (resumable downloads, delta updates later).

## 10. Performance work this unlocks

Things we cannot do on someone else's distro:

- Kernel config and version chosen for KVM + GPU (preempt model, hugepages,
  IOMMU defaults, isolated cores for vCPUs).
- GPU driver versions pinned to what we tested against nesbox.
- CPU governor, IRQ affinity and NIC settings (fq_codel/cake) set by default.
- One known configuration to benchmark, instead of every user's machine.

Each of these needs a before/after number from `nesbox/docs/BENCHMARKS.md`
before it ships as a default.

## 11. Hardware support

v1 targets x86_64, UEFI, TPM 2.0, one discrete GPU from AMD or NVIDIA. Machines
without a TPM install but cannot host other teams' boxes. Anything else is out
until someone needs it.

## 12. Milestones

1. **Bootable image:** mkosi build, UKI, verity root, boots on a build host into
   neslet, nothing else. Reproducible from one command in CI.
2. **Runs a box:** nesbox + GPU stack in the image; a guest renders a frame on
   the GPU test boxes.
3. **Updates and rollback:** A/B with a CI test that forces rollback.
4. **Zero-touch install:** installer + enrollment code + dashboard binding.
5. **Remote access:** nessh sessions over the neslet connection; no listener.
6. **Storage pool and disk add.**
7. **Measured boot + attestation**, required before any host serves another
   team.

## 13. Open questions (summary)

- License for our own code, and how the NVIDIA kernel module is shipped
  (prebuilt in the image vs. built on the host)
- Base build system (mkosi vs. Yocto vs. Bottlerocket fork)
- Whether BYO hosts serve other teams, which decides how early attestation lands
- Recovery path when a BYO host is offline
- Storage pool layout
- Who holds the image signing key, and how it is rotated
