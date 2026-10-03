# neslina

**The operating system a Nestri host runs. Nothing else.**

Install it on a machine with a GPU and the machine becomes a Nestri host: it
boots, registers itself, and is ready to run boxes. You do not log into it, you
do not configure it, and you do not update it by hand.

```
install neslina  →  machine boots  →  host appears in your team  →  boxes run
```

Same idea as AWS's Bottlerocket or Talos Linux, built for one workload: GPU
virtual machines streamed over the network.

## Why

nesbox, vDRM and virtio-nvgpu all give a guest a path to the host's GPU driver.
That path will never be provably safe; `nesbox/docs/SECURITY.md` says so plainly.
Today those components run on whatever distro the host owner installed, with
whatever else is on it. We cannot reason about the security of a machine we do
not control.

neslina moves the boundary one level down. If the guest escapes nesbox, it lands
on a host that:

- has no shell, no package manager, no compilers, no SSH daemon
- mounts its root filesystem read-only and verifies it at boot
- runs only the processes we ship
- is reachable for debugging only through our own servers, on our terms

And because we own the whole image, we can ship things we cannot ship today:
kernel and driver updates on our schedule, a kernel tuned for this workload,
and a GPU driver version we have actually tested.

## What it is

- An immutable, image-based Linux. One image, A/B partitions, atomic update,
  automatic rollback on a failed boot.
- `neslet` (the host agent), nesbox, and the GPU drivers. That is the userspace.
- A data partition for everything that must survive an update: box disks,
  caches, and the host's identity.
- No GUI, no console login, no local admin.

## What it is not

- A general-purpose distro. If you want to run something else on the machine,
  this is not for that machine.
- A gaming OS. Gaming is one workload on it.

## Status

Design. Nothing builds yet. Read [`docs/DESIGN.md`](docs/DESIGN.md).

## License

Open source. The image ships the Linux kernel and other GPL software, so its
source has to be public anyway. The exact license for our own code is not
decided yet.
