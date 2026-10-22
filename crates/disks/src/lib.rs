//! The host's disks: which ones there are, which ones neslina owns, and making
//! a disk into a state or games disk.
//!
//! neslina finds its own partitions by GPT name, as stage 1 finds the boot
//! medium: `NESLINA-STATE` and `NESLINA-GAMES`. Kernel names can change
//! between boots; partition names do not.

pub mod blockdev;
pub mod gpt;
pub mod setup;

/// GPT name of the state partition, mounted at `/var`.
pub const STATE: &str = "NESLINA-STATE";
/// GPT name of the games partition, mounted at `/var/neslina/games`.
pub const GAMES: &str = "NESLINA-GAMES";
