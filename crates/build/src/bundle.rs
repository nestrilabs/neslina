//! The install bundle: what a host needs to build its own slots offline.
//!
//! - `sysroot.erofs`: the toolchain, as `scripts/build-sysroot.sh` makes it.
//! - `source.erofs`: `repo/` (this repository: recipes, kernel config,
//!   neslina's own sources, a `VERSION` file, and the setup UI already built)
//!   and `store/` (exactly the archives and vendored crates the recipes name).
//!
//! The setup UI is built here, with Deno, from its lockfile: a host never has
//! a JavaScript toolchain, and `neslina-setup` embeds the built files.
//!
//! Both are read-only inputs to `neslina build`, so a host copies them to its
//! state disk and mounts them; nothing is unpacked.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::recipe::{Image, Recipe, Source};
use crate::sandbox::{Bind, Sandbox};
use crate::store::Store;

pub struct Options {
    pub repo: PathBuf,
    pub store: PathBuf,
    /// The unpacked sysroot, to run mkfs.erofs from.
    pub sysroot: PathBuf,
    /// `sysroot.erofs`, copied into the bundle.
    pub sysroot_image: PathBuf,
    pub work: PathBuf,
    pub out: PathBuf,
    pub version: String,
}

pub fn make(o: &Options) -> Result<()> {
    let stage = o.work.join("bundle");
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    let (repo, store_dir) = (stage.join("repo"), stage.join("store"));
    fs::create_dir_all(&repo)?;

    println!("collecting the repository");
    let listed = Command::new("git")
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .current_dir(&o.repo)
        .output()
        .context("running git ls-files")?;
    if !listed.status.success() {
        bail!("git ls-files failed in {}", o.repo.display());
    }
    for rel in listed.stdout.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let rel = Path::new(std::str::from_utf8(rel)?);
        let src = o.repo.join(rel);
        if !src.is_file() {
            continue; // deleted in the working tree
        }
        let dst = repo.join(rel);
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::copy(&src, &dst).with_context(|| format!("copying {}", rel.display()))?;
    }
    fs::write(repo.join("VERSION"), format!("{}\n", o.version))?;

    println!("building the setup UI");
    let ui = o.repo.join("ui");
    for args in [&["install", "--frozen"][..], &["task", "build"][..]] {
        let status = Command::new("deno")
            .args(args)
            .current_dir(&ui)
            .stdout(std::process::Stdio::null())
            .status()
            .context("running deno; the setup UI is built with it")?;
        if !status.success() {
            bail!("deno {} failed in {}", args.join(" "), ui.display());
        }
    }
    copy_tree(&ui.join("build"), &repo.join("ui/build"))?;

    println!("collecting sources");
    let store = Store { dir: o.store.clone() };
    let bundled = Store { dir: store_dir.clone() };
    let recipes = o.repo.join("recipes");
    for name in Image::load(&recipes)?.all() {
        match Recipe::load(&recipes, name)?.source {
            Some(Source::Url { sha256, .. }) => {
                let from = store.archive(&sha256);
                if !from.exists() {
                    bail!("{name}: source not in the store; run `neslina fetch`");
                }
                fs::create_dir_all(bundled.archive(&sha256).parent().unwrap())?;
                fs::copy(&from, bundled.archive(&sha256))?;
            }
            Some(Source::Path { .. }) if store.vendor(name).exists() => {
                copy_tree(&store.vendor(name), &bundled.vendor(name))?;
            }
            _ => {}
        }
    }

    fs::create_dir_all(&o.out)?;
    println!("making source.erofs");
    let sandbox = Sandbox {
        sysroot: &o.sysroot,
        scratch: o.work.join(".root"),
        binds: vec![Bind::ro(&stage, "/stage"), Bind::rw(&o.out, "/out")],
        overlays: Vec::new(),
        env: Vec::new(),
        workdir: PathBuf::from("/"),
    };
    sandbox
        .run("rm -f /out/source.erofs\nmkfs.erofs --quiet --all-root -T 0 -z lz4hc,12 /out/source.erofs /stage")
        .context("making source.erofs")?;
    println!("copying sysroot.erofs");
    fs::copy(&o.sysroot_image, o.out.join("sysroot.erofs"))
        .with_context(|| format!("copying {}", o.sysroot_image.display()))?;
    fs::remove_dir_all(&stage)?;
    println!("bundle {} in {}", o.version, o.out.display());
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}
