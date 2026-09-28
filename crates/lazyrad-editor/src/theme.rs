#![forbid(unsafe_code)]

//! The editor's palette, derived from xui's semantic [`Theme`] tokens.
//!
//! Deriving every colour from the theme means the editor gets light and dark
//! mode for free, exactly as PLAN.md §5 wants.

use xui_core::Color;
use xui_core::theme::Theme;

/// The colours the editor paints with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorTheme {
    /// The text area background.
    pub background: Color,
    /// Body text.
    pub text: Color,
    /// The gutter's background.
    pub gutter_background: Color,
    /// Line numbers and gutter marks.
    pub gutter_text: Color,
    /// The current line's highlight.
    pub current_line: Color,
    /// The selection fill while the editor has focus.
    pub selection: Color,
    /// The selection fill while it does not.
    pub selection_unfocused: Color,
    /// The caret.
    pub caret: Color,
    /// The editor's border.
    pub border: Color,
    /// The border while focused.
    pub border_focused: Color,
    /// Error markers (squiggles and line tints).
    pub error: Color,
    /// Warning markers.
    pub warning: Color,
    /// A breakpoint dot in the gutter.
    pub breakpoint: Color,
    /// The scrollbar thumb.
    pub scrollbar: Color,
    /// The scrollbar track.
    pub scrollbar_track: Color,
}

impl EditorTheme {
    /// Derives the palette from xui's semantic tokens.
    pub fn from_theme(theme: Theme) -> EditorTheme {
        EditorTheme {
            background: theme.input_background,
            text: theme.text,
            gutter_background: theme.surface,
            gutter_text: theme.text_secondary,
            current_line: theme.hover,
            selection: theme.selection,
            selection_unfocused: theme.selection_unfocused,
            caret: theme.text,
            border: theme.input_border,
            border_focused: theme.border_focused,
            error: theme.danger,
            warning: theme.warning,
            breakpoint: theme.danger,
            scrollbar: theme.scrollbar,
            scrollbar_track: theme.scrollbar_track,
        }
    }
}

impl Default for EditorTheme {
    fn default() -> EditorTheme {
        EditorTheme::from_theme(Theme::light())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_and_dark_palettes_differ() {
        let light = EditorTheme::from_theme(Theme::light());
        let dark = EditorTheme::from_theme(Theme::dark());
        assert_ne!(light.text, dark.text);
        assert_ne!(light.background, dark.background);
    }

    #[test]
    fn the_selection_uses_the_theme_selection_token() {
        let theme = Theme::light();
        assert_eq!(EditorTheme::from_theme(theme).selection, theme.selection);
    }
}
