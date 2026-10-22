//! The source store: every download a recipe pins, kept by its sha256, plus
//! vendored crates for neslina's own code.
//!
//! `neslina fetch` fills it, on a machine with network access. A build only
//! reads it, so a host builds with no network, from what the install medium
//! carried.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::recipe::{Recipe, Source, UNPINNED};

pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn archive(&self, sha256: &str) -> PathBuf {
        self.dir.join("sha256").join(sha256)
    }

    pub fn vendor(&self, recipe: &str) -> PathBuf {
        self.dir.join("vendor").join(recipe)
    }

    /// Makes sure everything this recipe needs is in the store. With `pin`,
    /// a download still pinned to zeros is fetched and its hash written into
    /// the recipe file at `recipe_path`.
    pub fn fetch(&self, recipe: &Recipe, recipe_path: &Path, repo: &Path, pin: bool) -> Result<()> {
        match &recipe.source {
            None => Ok(()),
            Some(Source::Url { url, sha256, .. }) if sha256 == UNPINNED => {
                if !pin {
                    bail!("{} is not pinned yet; run `neslina fetch --pin`", recipe.name);
                }
                let got = self.download(&recipe.name, url, None)?;
                let text = fs::read_to_string(recipe_path)?;
                fs::write(recipe_path, text.replacen(UNPINNED, &got, 1))?;
                println!("pinned {} to sha256 {got}", recipe.name);
                Ok(())
            }
            Some(Source::Url { url, sha256, .. }) => self.download(&recipe.name, url, Some(sha256)).map(drop),
            Some(Source::Path { path }) => {
                let dir = repo.join(path);
                if dir.join("Cargo.lock").exists() {
                    self.vendor_crates(&recipe.name, &dir)?;
                }
                Ok(())
            }
        }
    }

    /// Downloads into the store, checking the hash when one is pinned, and
    /// returns the hash.
    fn download(&self, name: &str, url: &str, sha256: Option<&str>) -> Result<String> {
        if let Some(sha) = sha256
            && self.archive(sha).exists()
        {
            return Ok(sha.to_owned());
        }
        let incoming = self.dir.join("incoming");
        fs::create_dir_all(&incoming)?;
        fs::create_dir_all(self.dir.join("sha256"))?;
        println!("fetching {name}: {url}");
        let partial = incoming.join(name);
        let mut reader = ureq::get(url).call().with_context(|| format!("requesting {url}"))?.into_body().into_reader();
        let mut out = File::create(&partial)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = reader.read(&mut buf).with_context(|| format!("downloading {url}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
        out.sync_all()?;
        let got = hex(&hasher.finalize());
        if let Some(sha) = sha256
            && got != sha
        {
            fs::remove_file(&partial).ok();
            bail!("{name}: {url} has sha256 {got}, the recipe pins {sha}");
        }
        fs::rename(&partial, self.archive(&got))?;
        Ok(got)
    }

    /// `cargo vendor` into the store. Re-run on every fetch: it is cheap
    /// when nothing changed, and the lock file decides what it holds.
    fn vendor_crates(&self, name: &str, dir: &Path) -> Result<()> {
        let dest = self.vendor(name);
        println!("vendoring crates for {name}");
        let status = Command::new("cargo")
            .args(["vendor", "--locked", "--quiet", "--versioned-dirs"])
            .arg(&dest)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .status()
            .context("running cargo vendor")?;
        if !status.success() {
            bail!("cargo vendor for {name} failed: {status}");
        }
        Ok(())
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut f, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}
