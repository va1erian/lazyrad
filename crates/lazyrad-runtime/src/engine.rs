#![forbid(unsafe_code)]

//! The Rhai engine host, re-exported from [`xui_rhai`].
//!
//! The engine setup lives in the reusable `xui-rhai` crate; this module keeps
//! the `lazyrad_runtime::engine` path its callers already use. The LazyRAD
//! standard library plugs into it through
//! [`EngineSetup`], implemented for
//! [`StdlibContext`](crate::stdlib::StdlibContext).

pub use xui_rhai::engine::*;
