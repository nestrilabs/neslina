//! Building one slot: each component, the root image, then the kernel.
//!
//! Every step runs in the sandbox, inside the sysroot. A recipe's script sees:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `SRC` | the source, at `/src` |
//! | `BUILD` | `/build`, kept between runs for incremental builds |
//! | `DEST` | `/dest`, emptied before each run; what the component installs |
//! | `REPO` | the neslina repository, read-only, at `/repo` |
//! | `VENDOR` | vendored crates, for a `path` source with a `Cargo.lock` |
//! | `JOBS` | parallel jobs |
//! | `NESLINA_CPU` | `generic` or `native` |
//! | `MARCH` | `x86-64-v3` or `native` |
//! | `CC`, `CXX`, `AR`, `NM`, `RANLIB`, `CFLAGS`, `CXXFLAGS`, `LDFLAGS` | clang, mold, `-march=$MARCH` |
//!
//! Components put everything for the root image under `$DEST`, and anything
//! for the initramfs under `$DEST/boot/initramfs`. The kernel recipe also
//! gets `/stage` (the initramfs list and `slot.config`) and `SLOT`, and
//! installs `$DEST/kernel.efi`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::recipe::{Image, Recipe, Source};
use crate::sandbox::{self, Bind, Sandbox};
use crate::store::{Store, sha256_file};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cpu {
    /// x86-64-v3: CI images and the rescue slot.
    Generic,
    /// The CPU doing the build.
    Native,
}

impl Cpu {
    pub fn name(self) -> &'static str {
        match self {
            Cpu::Generic => "generic",
            Cpu::Native => "native",
        }
    }

    pub fn march(self) -> &'static str {
        match self {
            Cpu::Generic => "x86-64-v3",
            Cpu::Native => "native",
        }
    }
}

/// A slot that cannot boot, to prove rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broken {
    /// The kernel expects the wrong root image hash; stage 1 fails and the kernel panics.
    Panic,
    /// Stage 1 stops after arming the watchdog, which then resets the board.
    Hang,
}

pub struct Options {
    pub repo: PathBuf,
    pub store: PathBuf,
    pub sysroot: PathBuf,
    pub work: PathBuf,
    pub out: PathBuf,
    pub cpu: Cpu,
    pub slot: String,
    pub version: String,
    pub broken: Option<Broken>,
    pub jobs: usize,
}

/// The kernel's command line. The log goes to serial and to a screen, if one
/// is attached. Our programs log through /dev/kmsg, which is rate limited
/// unless `printk.devkmsg=on`.
fn cmdline(slot: &str) -> String {
    format!(
        "console=tty0 console=ttyS0,115200 panic=10 printk.devkmsg=on transparent_hugepage_shmem=advise neslina.slot={slot}"
    )
}

pub fn build(o: &Options) -> Result<()> {
    if !o.slot.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') || o.slot.is_empty() {
        bail!("slot names are lowercase letters, digits and dashes");
    }
    let recipes_dir = o.repo.join("recipes");
    let image = Image::load(&recipes_dir)?;
    let store = Store { dir: o.store.clone() };
    let work = o.work.join(o.cpu.name());
    fs::create_dir_all(&work)?;

    for name in &image.components {
        let recipe = Recipe::load(&recipes_dir, name)?;
        build_component(o, &store, &work, &recipe, Vec::new(), Vec::new())?;
    }

    let stage = work.join("stage").join(&o.slot);
    let root_sha = assemble(o, &work, &image, &stage)?;
    write_stage(o, &stage, &root_sha)?;

    let kernel = Recipe::load(&recipes_dir, &image.kernel)?;
    build_component(
        o,
        &store,
        &work,
        &kernel,
        vec![Bind::ro(&stage, "/stage")],
        vec![("SLOT".into(), o.slot.clone())],
    )?;

    let out = o.out.join(&o.slot);
    fs::create_dir_all(&out)?;
    let efi = out.join(format!("{}.efi", o.slot));
    let erofs = out.join(format!("{}.erofs", o.slot));
    fs::copy(work.join(&kernel.name).join("dest/kernel.efi"), &efi).context("copying the kernel")?;
    fs::copy(stage.join("root.erofs"), &erofs).context("copying the root image")?;
    println!("built slot {} ({}):", o.slot, o.cpu.name());
    println!("{}", efi.display());
    println!("{} (sha256 {root_sha})", erofs.display());
    Ok(())
}

fn base_env(o: &Options) -> Vec<(String, String)> {
    let march = o.cpu.march();
    let cflags = format!("-O2 -pipe -march={march}");
    [
        ("SRC", "/src".to_owned()),
        ("BUILD", "/build".to_owned()),
        ("DEST", "/dest".to_owned()),
        ("REPO", "/repo".to_owned()),
        ("JOBS", o.jobs.to_string()),
        ("NESLINA_CPU", o.cpu.name().to_owned()),
        ("MARCH", march.to_owned()),
        ("CC", "clang".to_owned()),
        ("CXX", "clang++".to_owned()),
        ("AR", "llvm-ar".to_owned()),
        ("NM", "llvm-nm".to_owned()),
        ("RANLIB", "llvm-ranlib".to_owned()),
        ("CFLAGS", cflags.clone()),
        ("CXXFLAGS", cflags),
        ("LDFLAGS", "-fuse-ld=mold".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// Creates a directory the sandbox writes to, owned by the uid builds run as.
fn build_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    // SAFETY: plain syscall.
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::chown(path, Some(sandbox::BUILD_UID), Some(sandbox::BUILD_UID))?;
    }
    Ok(())
}

fn sandbox<'a>(o: &'a Options, work: &Path, binds: Vec<Bind>, env: Vec<(String, String)>) -> Sandbox<'a> {
    let mut all = vec![Bind::ro(&o.repo, "/repo")];
    all.extend(binds);
    let mut full_env = base_env(o);
    full_env.extend(env);
    Sandbox { sysroot: &o.sysroot, scratch: work.join(".root"), binds: all, env: full_env, workdir: PathBuf::from("/") }
}

fn build_component(
    o: &Options,
    store: &Store,
    work: &Path,
    recipe: &Recipe,
    extra_binds: Vec<Bind>,
    extra_env: Vec<(String, String)>,
) -> Result<()> {
    let dir = work.join(&recipe.name);
    let (src, build, dest) = (dir.join("src"), dir.join("build"), dir.join("dest"));
    build_dir(&dir)?;
    build_dir(&build)?;
    if dest.exists() {
        fs::remove_dir_all(&dest).with_context(|| format!("emptying {}", dest.display()))?;
    }
    build_dir(&dest)?;

    let mut binds = vec![Bind::rw(&build, "/build"), Bind::rw(&dest, "/dest")];
    let mut env = extra_env;
    match &recipe.source {
        Source::Url { sha256, strip, .. } => {
            unpack(o, work, store, recipe, sha256, *strip, &dir)?;
            binds.push(Bind::rw(&src, "/src"));
        }
        Source::Path { path } => {
            binds.push(Bind::ro(o.repo.join(path), "/src"));
            let vendor = store.vendor(&recipe.name);
            if vendor.exists() {
                binds.push(Bind::ro(vendor, "/vendor"));
                env.push(("VENDOR".into(), "/vendor".into()));
            }
        }
    }
    binds.extend(extra_binds);

    println!("building {} {}", recipe.name, recipe.version);
    sandbox(o, work, binds, env).run(&recipe.build.script).with_context(|| format!("building {}", recipe.name))
}

/// Unpacks a downloaded source unless the same one is already unpacked. The
/// stamp records which archive the tree came from.
fn unpack(
    o: &Options,
    work: &Path,
    store: &Store,
    recipe: &Recipe,
    sha256: &str,
    strip: u32,
    dir: &Path,
) -> Result<()> {
    let src = dir.join("src");
    let stamp = dir.join("src.stamp");
    let want = format!("{sha256} strip={strip}\n");
    if src.exists() && fs::read_to_string(&stamp).is_ok_and(|s| s == want) {
        return Ok(());
    }
    let archive = store.archive(sha256);
    if !archive.exists() {
        bail!("{}: source not in the store; run `neslina fetch`", recipe.name);
    }
    println!("unpacking {} {}", recipe.name, recipe.version);
    // A build directory made from other sources would be reused against
    // these; start both over.
    for stale in [&src, &dir.join("build")] {
        if stale.exists() {
            fs::remove_dir_all(stale).with_context(|| format!("removing {}", stale.display()))?;
        }
    }
    build_dir(&src)?;
    build_dir(&dir.join("build"))?;
    let binds = vec![Bind::ro(archive, "/archive"), Bind::rw(&src, "/src")];
    sandbox(o, work, binds, Vec::new())
        .run(&format!("tar -xf /archive --strip-components={strip} -C /src"))
        .with_context(|| format!("unpacking {}", recipe.name))?;
    fs::write(&stamp, want)?;
    Ok(())
}

/// Merges the components into the root image. Returns its sha256.
fn assemble(o: &Options, work: &Path, image: &Image, stage: &Path) -> Result<String> {
    build_dir(stage)?;
    let mut binds = vec![Bind::rw(stage, "/stage")];
    for name in &image.components {
        binds.push(Bind::ro(work.join(name).join("dest"), format!("/parts/{name}")));
    }
    let script = format!(
        r#"
rm -rf /stage/root /stage/initramfs /stage/root.erofs
mkdir -p /stage/root /stage/initramfs
for part in /parts/*; do
    cp -a --no-preserve=ownership "$part"/. /stage/root/
done
if [ -d /stage/root/boot/initramfs ]; then
    cp -a /stage/root/boot/initramfs/. /stage/initramfs/
fi
rm -rf /stage/root/boot
mkdir -p /stage/root/{{dev,proc,sys,run,tmp,var,etc}}
cat >/stage/root/etc/os-release <<EOF
NAME=neslina
ID=neslina
PRETTY_NAME="neslina"
VERSION_ID={version}
HOME_URL=https://github.com/nestrilabs/neslina
EOF
# Fixed timestamps and root ownership: the same input gives the same image.
mkfs.erofs --quiet --all-root -T 0 -z lz4hc,12 /stage/root.erofs /stage/root
"#,
        version = o.version
    );
    println!("assembling the root image");
    sandbox(o, work, binds, Vec::new()).run(&script).context("assembling the root image")?;
    Ok(sha256_file(&stage.join("root.erofs"))?)
}

/// What the kernel build embeds: the slot file stage 1 reads, the initramfs
/// list, and the config fragment with the command line.
fn write_stage(o: &Options, stage: &Path, root_sha: &str) -> Result<()> {
    let mut slot = format!("slot={}\nsha256={root_sha}\n", o.slot);
    match o.broken {
        Some(Broken::Panic) => {
            println!("building slot {} broken on purpose: its kernel expects the wrong root image", o.slot);
            slot = format!("slot={}\nsha256={}\n", o.slot, "0".repeat(64));
        }
        Some(Broken::Hang) => {
            println!("building slot {} broken on purpose: stage 1 hangs until the watchdog resets", o.slot);
            slot.push_str("test=hang\n");
        }
        None => {}
    }
    fs::write(stage.join("slot"), slot)?;

    // gen_init_cpio's format, so device nodes need no root. Paths are as the
    // kernel build sees them, under /stage.
    let mut list = String::from(
        "dir /dev 0755 0 0\nnod /dev/console 0600 0 0 c 5 1\ndir /proc 0755 0 0\ndir /sys 0755 0 0\n\
         dir /medium 0755 0 0\ndir /newroot 0755 0 0\ndir /ram 0700 0 0\ndir /neslina 0755 0 0\n\
         file /neslina/slot /stage/slot 0644 0 0\n",
    );
    let initramfs = stage.join("initramfs");
    if !initramfs.join("init").exists() {
        bail!("no component installed boot/initramfs/init");
    }
    list_tree(&initramfs, &initramfs, &mut list)?;
    fs::write(stage.join("initramfs.list"), list)?;

    fs::write(
        stage.join("slot.config"),
        format!("CONFIG_CMDLINE=\"{}\"\nCONFIG_INITRAMFS_SOURCE=\"/stage/initramfs.list\"\n", cmdline(&o.slot)),
    )?;
    Ok(())
}

fn list_tree(base: &Path, dir: &Path, list: &mut String) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let rel = path.strip_prefix(base)?;
        let mode = e.metadata()?.permissions().mode() & 0o7777;
        let kind = e.file_type()?;
        if kind.is_dir() {
            list.push_str(&format!("dir /{} {mode:o} 0 0\n", rel.display()));
            list_tree(base, &path, list)?;
        } else if kind.is_file() {
            list.push_str(&format!("file /{} /stage/initramfs/{} {mode:o} 0 0\n", rel.display(), rel.display()));
        } else {
            bail!("{}: only files and directories go in the initramfs", path.display());
        }
    }
    Ok(())
}
