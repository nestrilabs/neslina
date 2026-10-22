//! `neslina`: the one command for working with neslina.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use neslina_build::bundle;
use neslina_build::pipeline::{self, Broken, Cpu, Options};
use neslina_build::recipe::{Image, Recipe};
use neslina_build::store::Store;

#[derive(Parser)]
#[command(name = "neslina", version, about = "Build and manage a neslina host")]
struct Cli {
    /// The neslina repository: recipes, kernel config and our own sources.
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    /// Where downloaded sources and vendored crates are kept.
    #[arg(long, global = true)]
    store: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download every pinned source and vendor crates. Needs network.
    Fetch {
        /// Download sources pinned to zeros and write their hash into the recipe.
        #[arg(long)]
        pin: bool,
    },
    /// Build one slot: its root image and the kernel that boots it.
    Build(BuildArgs),
    /// Make the install bundle a host builds its own slots from.
    Bundle {
        /// The toolchain image (scripts/build-sysroot.sh).
        #[arg(long)]
        sysroot_image: Option<PathBuf>,
        /// The unpacked sysroot, to run mkfs.erofs from.
        #[arg(long)]
        sysroot: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
struct BuildArgs {
    #[arg(long, value_enum, default_value_t = CpuArg::Generic)]
    cpu: CpuArg,
    #[arg(long, default_value = "rescue")]
    slot: String,
    /// The unpacked build sysroot (scripts/build-sysroot.sh).
    #[arg(long)]
    sysroot: Option<PathBuf>,
    /// Kept between builds, for incremental rebuilds.
    #[arg(long)]
    work: Option<PathBuf>,
    /// Slots are written to <out>/<slot>/.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Build a slot that fails on purpose, to test rollback.
    #[arg(long, value_enum)]
    broken: Option<BrokenArg>,
    #[arg(long)]
    jobs: Option<usize>,
}

#[derive(Clone, Copy, ValueEnum)]
enum CpuArg {
    Generic,
    Native,
}

#[derive(Clone, Copy, ValueEnum)]
enum BrokenArg {
    Panic,
    Hang,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo = std::fs::canonicalize(&cli.repo).with_context(|| format!("finding {}", cli.repo.display()))?;
    let output = repo.join("output");
    let store = Store { dir: cli.store.unwrap_or_else(|| output.join("store")) };

    match cli.command {
        Command::Fetch { pin } => {
            let recipes = repo.join("recipes");
            for name in Image::load(&recipes)?.all() {
                store.fetch(&Recipe::load(&recipes, name)?, &Recipe::path(&recipes, name), &repo, pin)?;
            }
            println!("store is complete: {}", store.dir.display());
            Ok(())
        }
        Command::Bundle { sysroot_image, sysroot, out } => bundle::make(&bundle::Options {
            sysroot_image: sysroot_image.unwrap_or_else(|| output.join("sysroot/sysroot.erofs")),
            sysroot: sysroot.unwrap_or_else(|| output.join("sysroot/root")),
            work: output.join("work"),
            out: out.unwrap_or_else(|| output.join("bundle")),
            version: version(&repo),
            store: store.dir,
            repo,
        }),
        Command::Build(b) => {
            let opts = Options {
                sysroot: b.sysroot.unwrap_or_else(|| output.join("sysroot/root")),
                work: b.work.unwrap_or_else(|| output.join("work")),
                out: b.out.unwrap_or_else(|| output.join("image")),
                cpu: match b.cpu {
                    CpuArg::Generic => Cpu::Generic,
                    CpuArg::Native => Cpu::Native,
                },
                slot: b.slot,
                version: version(&repo),
                broken: b.broken.map(|b| match b {
                    BrokenArg::Panic => Broken::Panic,
                    BrokenArg::Hang => Broken::Hang,
                }),
                jobs: b.jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get())),
                store: store.dir,
                repo,
            };
            pipeline::build(&opts)
        }
    }
}

/// The repository's `VERSION` file (a bundle carries one), otherwise
/// `git describe`, otherwise the crate version.
fn version(repo: &std::path::Path) -> String {
    if let Ok(v) = std::fs::read_to_string(repo.join("VERSION")) {
        return v.trim().to_owned();
    }
    std::process::Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}
