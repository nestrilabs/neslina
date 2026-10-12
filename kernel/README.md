# Host kernel patches

Patches the host kernel needs that no distro kernel carries yet. Until neslina
builds its own image these are applied by hand to a test host; once it does,
they are part of its kernel build. Each one should also go upstream: carrying a
patch here is a stopgap, not the goal.

Base: CachyOS `cachyos-7.2.9-1` (<https://github.com/CachyOS/linux>).

## Working on them

The patches are `git format-patch` output from a branch on that tag. The tree
lives under `output/`, which git ignores:

```bash
git clone https://github.com/CachyOS/linux.git output/linux
git -C output/linux switch -c neslina-7.2.9 cachyos-7.2.9-1
git -C output/linux am ../../kernel/patches/*.patch
```

Change the branch, then regenerate:

```bash
rm kernel/patches/*.patch
git -C output/linux format-patch -o ../../kernel/patches cachyos-7.2.9-1..HEAD
```

## The patches

| Patch | Why | Status |
|---|---|---|
| `0001-cgroup-dmem-respect-protection-when-the-common-ances` | `dmem.min` protects nothing between cgroups that are both children of the root | Compiles. Not yet booted. Not sent upstream. |

### 0001: dmem protection between children of the root

When a protected allocation evicts, TTM measures protection from the closest
cgroup the two share, and `dmem_cgroup_state_evict_valuable()` treats every
buffer as evictable when that cgroup is the root. Box cgroups sit directly
under the root, so one box could evict another box's protected VRAM while a
third box sat gigabytes over its share.

neslet is not meant to depend on this patch: the plan is for it to put the box
cgroups under a shared parent, which avoids the bug on any kernel (measured in
the lab; not built into neslet yet). The patch matters for anyone else
whose dmem cgroups sit directly under the root, and it is the fix that should
go upstream.
