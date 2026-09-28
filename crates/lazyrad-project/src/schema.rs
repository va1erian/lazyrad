#![forbid(unsafe_code)]

//! The LazyRAD control catalog: the shared `xui-form` catalog plus the VB-style
//! control names LazyRAD exposes.
//!
//! The heavy lifting lives in [`xui_form::Catalog`]. LazyRAD only adds aliases,
//! so `CommandButton` resolves to the portable `Button` spec without copying it.
//! The designer's toolbox, the property grid and the code editor's completion
//! all read the same catalog.

use xui_form::Catalog;

/// The LazyRAD catalog: [`Catalog::xui`] with the VB6 control names as aliases.
///
/// | VB name | Portable kind |
/// |---|---|
/// | `CommandButton` | `Button` |
/// | `TextBox` | `Edit` |
/// | `Frame` | `GroupBox` |
/// | `ListBox` | `ListView` |
/// | `OptionButton` | `RadioGroup` |
pub fn lazyrad_catalog() -> Catalog {
    let mut catalog = Catalog::xui();
    catalog.alias("CommandButton", "Button");
    catalog.alias("TextBox", "Edit");
    catalog.alias("Frame", "GroupBox");
    catalog.alias("ListBox", "ListView");
    catalog.alias("OptionButton", "RadioGroup");
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vb_names_resolve_to_portable_kinds() {
        let catalog = lazyrad_catalog();
        for (vb, kind) in [
            ("CommandButton", "Button"),
            ("TextBox", "Edit"),
            ("Frame", "GroupBox"),
            ("ListBox", "ListView"),
            ("OptionButton", "RadioGroup"),
        ] {
            assert_eq!(catalog.resolve(vb), Some(kind), "{vb}");
            assert!(catalog.get(vb).is_some(), "{vb} resolves");
        }
        // The portable kinds remain available under their own names.
        assert!(catalog.get("Button").is_some());
        assert!(catalog.get("ListView").is_some());
    }
}
