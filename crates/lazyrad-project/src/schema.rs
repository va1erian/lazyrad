#![forbid(unsafe_code)]

//! The LazyRAD control catalog.
//!
//! LazyRAD uses the portable [`xui_form::Catalog`] as is: control kinds and
//! property names are xui's own (`Button`, `Edit`, `text`, `selected`…), with
//! no VB6 aliases. VB6 inspires the IDE's workflow, not the names in code
//! (PLAN.md §1). This function stays the single place LazyRAD adds its own
//! kinds or catalog tweaks, so the designer's toolbox, the property grid and
//! the code editor's completion all read the same catalog.

use xui_form::Catalog;

/// The LazyRAD catalog: currently exactly [`Catalog::xui`].
pub fn lazyrad_catalog() -> Catalog {
    Catalog::xui()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_uses_the_portable_kind_names() {
        let catalog = lazyrad_catalog();
        for kind in ["Button", "Edit", "GroupBox", "ListView", "RadioGroup"] {
            assert!(catalog.get(kind).is_some(), "{kind}");
        }
        for vb in [
            "CommandButton",
            "TextBox",
            "Frame",
            "ListBox",
            "OptionButton",
        ] {
            assert!(catalog.get(vb).is_none(), "no VB alias for {vb}");
        }
    }
}
