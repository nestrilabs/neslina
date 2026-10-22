//! Recipes: one TOML file per component in `recipes/`, plus `recipes/image.toml`
//! naming which components make up a slot.
//!
//! ```toml
//! name = "linux"
//! version = "cachyos-7.3.0-2"
//!
//! [source]
//! url = "https://…/linux-cachyos-7.3.0-2.tar.gz"
//! sha256 = "…"
//! strip = 1            # leading path components to drop when unpacking
//!
//! [build]
//! script = """
//! make -j"$JOBS" …
//! """
//! ```
//!
//! A source is either a pinned download (`url` and `sha256`) or a directory
//! in the repository (`path`, relative to the repository root), used for
//! neslina's own code. A `path` source with a `Cargo.lock` gets its crates
//! vendored by `neslina fetch`, so the build itself needs no network. A recipe
//! with no source takes what it installs from the sysroot (glibc).
//!
//! A download pinned to 64 zeros is not pinned yet: `neslina fetch --pin`
//! downloads it and writes the real hash into the recipe.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub source: Option<Source>,
    pub build: Build,
}

#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Source {
    Url {
        url: String,
        sha256: String,
        #[serde(default)]
        strip: u32,
    },
    Path {
        path: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    /// Run with bash inside the sysroot. See `pipeline` for its environment.
    pub script: String,
}

/// `recipes/image.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    /// Built in this order; their outputs together are the root image.
    pub components: Vec<String>,
    /// Built last, with the root image's hash and the initramfs.
    pub kernel: String,
}

/// The placeholder for a download not pinned yet.
pub const UNPINNED: &str = "0000000000000000000000000000000000000000000000000000000000000000";

impl Recipe {
    pub fn path(recipes: &Path, name: &str) -> PathBuf {
        recipes.join(format!("{name}.toml"))
    }

    pub fn load(recipes: &Path, name: &str) -> Result<Self> {
        let path = Self::path(recipes, name);
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let recipe: Recipe = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if recipe.name != name {
            bail!("{} is named {:?}, expected {name:?}", path.display(), recipe.name);
        }
        if let Some(Source::Url { sha256, .. }) = &recipe.source
            && (sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            bail!("{}: sha256 must be 64 hex digits", path.display());
        }
        Ok(recipe)
    }
}

impl Image {
    pub fn load(recipes: &Path) -> Result<Self> {
        let path = recipes.join("image.toml");
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn all(&self) -> impl Iterator<Item = &str> {
        self.components.iter().map(String::as_str).chain(std::iter::once(self.kernel.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_source_kinds() {
        let url: Recipe = toml::from_str(
            r#"
            name = "x"
            version = "1"
            [source]
            url = "https://example.org/x.tar.gz"
            sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
            strip = 1
            [build]
            script = "true"
            "#,
        )
        .unwrap();
        assert!(matches!(url.source, Some(Source::Url { strip: 1, .. })));

        let path: Recipe = toml::from_str(
            r#"
            name = "y"
            version = "0"
            [source]
            path = "."
            [build]
            script = "true"
            "#,
        )
        .unwrap();
        assert!(matches!(path.source, Some(Source::Path { .. })));
    }

    #[test]
    fn rejects_unknown_fields() {
        let r: Result<Recipe, _> = toml::from_str(
            r#"
            name = "x"
            version = "1"
            typo = true
            [source]
            path = "."
            [build]
            script = "true"
            "#,
        );
        assert!(r.is_err());
    }
}
