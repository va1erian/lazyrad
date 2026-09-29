#![forbid(unsafe_code)]

//! The operating system's light/dark preference.

/// Whether the operating system currently prefers a dark theme.
///
/// A detection failure is treated as light, the same fallback the rest of the
/// IDE uses. Platforms the detection crate does not cover (Linux, without a
/// desktop portal wired up) also fall back to light; `System` still differs
/// from a pinned `Light` in that a supporting desktop reports its preference.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn system_prefers_dark() -> bool {
    matches!(dark_light::detect(), Ok(dark_light::Mode::Dark))
}

/// No system theme detection on this platform; use light.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn system_prefers_dark() -> bool {
    portable_prefers_dark()
}

/// The portable fallback: light.
#[cfg_attr(any(target_os = "windows", target_os = "macos"), allow(dead_code))]
fn portable_prefers_dark() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_portable_fallback_is_light() {
        assert!(!super::portable_prefers_dark());
    }
}
