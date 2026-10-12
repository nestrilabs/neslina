//! Slots as UEFI boot entries.
//!
//! Each slot's kernel is an EFI file on the boot medium, started directly by
//! the firmware. A slot gets a `Boot####` entry pointing at it. Trying a slot
//! is setting `BootNext`, which the firmware uses once; making it the default
//! is moving its entry to the front of `BootOrder`.

pub mod efivar;
pub mod gpt;
pub mod loadopt;
