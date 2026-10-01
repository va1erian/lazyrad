#![forbid(unsafe_code)]

//! The operating system's light/dark preference, as the installed platform
//! reports it.

use lazyrad_runtime::platform;

/// Whether the operating system currently prefers a dark theme.
///
/// A platform that cannot answer (or none installed) is light, the same
/// fallback the rest of the IDE uses.
pub fn system_prefers_dark() -> bool {
    platform::current().prefers_dark()
}

#[cfg(test)]
mod tests {
    #[test]
    fn with_no_platform_installed_the_preference_is_light() {
        assert!(!super::system_prefers_dark());
    }
}
