//! Logging to the kernel log.
//!
//! Every neslina binary writes here, so stage 1, init and the services share one
//! ordered stream with the kernel's own messages, on the serial console and in
//! `dmesg`. There is no other log until the log service exists.
//!
//! Before `/dev/kmsg` exists (stage 1, before it mounts devtmpfs) lines go to
//! stderr instead, which the kernel points at the console.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error = 3,
    Warn = 4,
    Info = 6,
    Debug = 7,
}

static TAG: Mutex<&'static str> = Mutex::new("neslina");
static SINK: Mutex<Option<File>> = Mutex::new(None);

/// Sets the name every line is prefixed with.
pub fn init(tag: &'static str) {
    *TAG.lock().unwrap_or_else(|e| e.into_inner()) = tag;
}

pub fn write(level: Level, args: fmt::Arguments) {
    let tag = *TAG.lock().unwrap_or_else(|e| e.into_inner());
    let line = format!("<{}>{tag}: {args}\n", level as u8);

    let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
    if sink.is_none() {
        *sink = OpenOptions::new().write(true).open("/dev/kmsg").ok();
    }
    if let Some(f) = sink.as_mut() {
        // One write is one record; the kernel splits nothing.
        if f.write_all(line.as_bytes()).is_ok() {
            return;
        }
        *sink = None;
    }
    let _ = std::io::stderr().write_all(&line.as_bytes()[line.find('>').map_or(0, |i| i + 1)..]);
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::kmsg::write($crate::kmsg::Level::Error, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::kmsg::write($crate::kmsg::Level::Warn, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::kmsg::write($crate::kmsg::Level::Info, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { $crate::kmsg::write($crate::kmsg::Level::Debug, format_args!($($arg)*)) };
}
