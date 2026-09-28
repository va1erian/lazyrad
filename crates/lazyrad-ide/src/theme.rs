#![forbid(unsafe_code)]

//! Turning a [`ThemeChoice`] into the xui [`Theme`] palette the widgets paint
//! from (PLAN.md §9).

use xui_core::Theme as Palette;

use crate::settings::ThemeChoice;

/// Resolves `choice` to a palette.
///
/// `System` asks the platform for its light/dark preference through
/// `dark-light`; a platform that cannot answer falls back to light.
pub fn palette(choice: ThemeChoice) -> Palette {
    match choice {
        ThemeChoice::Light => Palette::light(),
        ThemeChoice::Dark => Palette::dark(),
        ThemeChoice::System => palette_for_dark(system_prefers_dark()),
    }
}

/// The palette for a resolved light/dark flag.
///
/// Split out from [`palette`] so the light/dark decision can be tested without
/// asking the platform.
fn palette_for_dark(dark: bool) -> Palette {
    if dark {
        Palette::dark()
    } else {
        Palette::light()
    }
}

/// Whether the operating system currently prefers a dark theme.
///
/// A detection failure is treated as light, the same fallback the rest of the
/// IDE uses. Platforms the detection crate does not cover (Linux, without a
/// desktop portal wired up) also fall back to light; `System` still differs
/// from a pinned `Light` in that a supporting desktop reports its preference.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn system_prefers_dark() -> bool {
    matches!(dark_light::detect(), Ok(dark_light::Mode::Dark))
}

/// No system theme detection on this platform; use light.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn system_prefers_dark() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_and_dark_choices_are_exact() {
        assert_eq!(palette(ThemeChoice::Light), Palette::light());
        assert_eq!(palette(ThemeChoice::Dark), Palette::dark());
    }

    #[test]
    fn the_dark_flag_selects_the_matching_palette() {
        assert_eq!(palette_for_dark(true), Palette::dark());
        assert_eq!(palette_for_dark(false), Palette::light());
    }
}
