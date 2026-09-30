#![forbid(unsafe_code)]

//! The IDE's platform seam (PLAN.md §12, "Porting LazyRAD").
//!
//! Since the LazyOS plan (P0) these modules are thin: they ask
//! [`lazyrad_runtime::platform::current`], and the desktop answers live in
//! [`host`] (feature `desktop`). A new platform implements
//! [`lazyrad_runtime::platform::Platform`] and installs it before the IDE starts.
//!
//! Everything the IDE needs from the operating system that xui cannot provide
//! lives behind this module, and no other IDE source file uses `cfg(target_os)`
//! or `cfg(windows)` (only `build.rs` may, for the Windows icon resource). A new
//! platform, LazyOS for one, implements just these submodules:
//!
//! * [`dialogs`]: native file/folder dialogs and the fatal-error box. The
//!   portable fallback (the `native-dialogs` feature off, or a platform with no
//!   dialog backend) answers "cancelled" to every request and prints errors to
//!   stderr, so the IDE never blocks on a dialog that cannot appear.
//! * [`theme`]: the operating system's light/dark preference. The portable
//!   fallback is light.
//! * [`process`]: launch flags for a child process, and the executable file
//!   naming convention. The portable fallback adds no flags and appends no
//!   suffix.
//! * [`config`]: the per-user configuration directory and the default monospace
//!   font. The portable fallback is "no config directory" (settings stay in
//!   memory) and DejaVu Sans Mono.

pub mod config;
pub mod dialogs;
#[cfg(feature = "desktop")]
pub mod host;
pub mod process;
pub mod theme;

pub use dialogs::show_fatal_error;
