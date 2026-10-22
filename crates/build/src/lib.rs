//! Building neslina: recipes, the source store, and sandboxed builds of a
//! slot inside the prebuilt sysroot (docs/DESIGN.md §6).

pub mod bundle;
pub mod pipeline;
pub mod recipe;
pub mod sandbox;
pub mod store;
