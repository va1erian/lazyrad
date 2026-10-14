//! Window size classes and the mobile-first responsive cascade.
//!
//! The classes follow Material 3's window width classes. A width is in
//! device-independent pixels (dp), which equal CSS pixels at 96 dpi, so the
//! numbers are the same on every renderer.
//!
//! | Class      | Width (dp)     |
//! |------------|----------------|
//! | `Compact`  | below 600      |
//! | `Medium`   | 600 to 839     |
//! | `Expanded` | 840 to 1199    |
//! | `Large`    | 1200 to 1599   |
//! | `XLarge`   | 1600 and above |
//!
//! Each boundary belongs to the class that starts there: exactly `600.0` is
//! `Medium`.

use std::collections::BTreeMap;
use std::fmt;

/// A window width class, ordered from the narrowest to the widest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SizeClass {
    /// Below 600 dp: a phone in portrait.
    Compact,
    /// 600 to 839 dp: a tablet in portrait, a foldable.
    Medium,
    /// 840 to 1199 dp: a tablet in landscape, a small desktop window.
    Expanded,
    /// 1200 to 1599 dp: a desktop window.
    Large,
    /// 1600 dp and above: a large desktop window.
    XLarge,
}

impl SizeClass {
    /// Every class, narrowest first.
    pub const ALL: [SizeClass; 5] = [
        SizeClass::Compact,
        SizeClass::Medium,
        SizeClass::Expanded,
        SizeClass::Large,
        SizeClass::XLarge,
    ];

    /// The class of a window `dp` wide.
    ///
    /// A width that is not a usable measurement (negative or NaN) is
    /// [`SizeClass::Compact`], the safe small-window layout. Positive infinity
    /// is [`SizeClass::XLarge`].
    #[must_use]
    pub fn from_width(dp: f32) -> Self {
        if dp.is_nan() || dp < Self::Medium.min_width() {
            Self::Compact
        } else if dp < Self::Expanded.min_width() {
            Self::Medium
        } else if dp < Self::Large.min_width() {
            Self::Expanded
        } else if dp < Self::XLarge.min_width() {
            Self::Large
        } else {
            Self::XLarge
        }
    }

    /// The narrowest width, in dp, that belongs to this class.
    #[must_use]
    pub const fn min_width(self) -> f32 {
        match self {
            Self::Compact => 0.0,
            Self::Medium => 600.0,
            Self::Expanded => 840.0,
            Self::Large => 1200.0,
            Self::XLarge => 1600.0,
        }
    }

    /// The lowercase name scripts see in `env.size_class`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Medium => "medium",
            Self::Expanded => "expanded",
            Self::Large => "large",
            Self::XLarge => "xlarge",
        }
    }
}

impl fmt::Display for SizeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A read-only view of a per-class table of partial property patches, resolved
/// **mobile-first** like CSS `min-width` media queries.
///
/// The table maps a [`SizeClass`] to a patch, a set of `property -> value`
/// overrides. To resolve a class, every patch at or below it applies in
/// ascending order and a wider class overrides a narrower one, **per
/// property**:
///
/// * an unspecified class inherits from the next smaller specified one;
/// * a class narrower than the smallest specified key gets nothing;
/// * a patch only replaces the properties it names, so `Medium: direction` and
///   `Expanded: gap` give `Expanded` both.
///
/// The view borrows the table, so a [`Node`](crate::Node)'s own `responsive`
/// field is used in place.
#[derive(Debug)]
pub struct Responsive<'a, V> {
    table: &'a BTreeMap<SizeClass, BTreeMap<String, V>>,
}

impl<V> Clone for Responsive<'_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for Responsive<'_, V> {}

impl<'a, V> Responsive<'a, V> {
    /// A view of `table`.
    #[must_use]
    pub const fn new(table: &'a BTreeMap<SizeClass, BTreeMap<String, V>>) -> Self {
        Self { table }
    }

    /// Every property the cascade sets at `class`, with its value.
    #[must_use]
    pub fn resolve(&self, class: SizeClass) -> BTreeMap<&'a str, &'a V> {
        let mut resolved = BTreeMap::new();
        for patch in self.table.range(..=class).map(|(_, patch)| patch) {
            for (property, value) in patch {
                resolved.insert(property.as_str(), value);
            }
        }
        resolved
    }

    /// The value the cascade gives `property` at `class`, if any patch at or
    /// below `class` names it.
    #[must_use]
    pub fn property(&self, class: SizeClass, property: &str) -> Option<&'a V> {
        self.table
            .range(..=class)
            .rev()
            .find_map(|(_, patch)| patch.get(property))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_at_every_boundary() {
        let cases = [
            (0.0, SizeClass::Compact),
            (599.99, SizeClass::Compact),
            (600.0, SizeClass::Medium),
            (839.99, SizeClass::Medium),
            (840.0, SizeClass::Expanded),
            (1199.99, SizeClass::Expanded),
            (1200.0, SizeClass::Large),
            (1599.99, SizeClass::Large),
            (1600.0, SizeClass::XLarge),
            (10_000.0, SizeClass::XLarge),
        ];
        for (width, expected) in cases {
            assert_eq!(SizeClass::from_width(width), expected, "{width} dp");
        }
    }

    #[test]
    fn unusable_widths_are_compact_and_never_panic() {
        for width in [-1.0, -600.0, f32::NEG_INFINITY, f32::NAN, -0.0] {
            assert_eq!(SizeClass::from_width(width), SizeClass::Compact);
        }
        assert_eq!(SizeClass::from_width(f32::INFINITY), SizeClass::XLarge);
    }

    #[test]
    fn classes_are_ordered_narrowest_first() {
        assert!(SizeClass::ALL.windows(2).all(|pair| pair[0] < pair[1]));
        for class in SizeClass::ALL {
            assert_eq!(SizeClass::from_width(class.min_width()), class);
        }
    }

    fn table(
        entries: &[(SizeClass, &[(&str, i32)])],
    ) -> BTreeMap<SizeClass, BTreeMap<String, i32>> {
        entries
            .iter()
            .map(|(class, patch)| {
                let patch = patch.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect();
                (*class, patch)
            })
            .collect()
    }

    #[test]
    fn unspecified_classes_inherit_from_the_next_smaller_one() {
        let table = table(&[(SizeClass::Medium, &[("gap", 1)])]);
        let responsive = Responsive::new(&table);
        assert_eq!(responsive.property(SizeClass::Compact, "gap"), None);
        assert_eq!(responsive.property(SizeClass::Medium, "gap"), Some(&1));
        for class in [SizeClass::Expanded, SizeClass::Large, SizeClass::XLarge] {
            assert_eq!(responsive.property(class, "gap"), Some(&1), "{class}");
        }
    }

    #[test]
    fn a_wider_class_overrides_per_property() {
        let table = table(&[
            (SizeClass::Medium, &[("gap", 1), ("fill", 1)]),
            (SizeClass::Expanded, &[("gap", 2), ("padding", 9)]),
        ]);
        let responsive = Responsive::new(&table);
        let at_medium = responsive.resolve(SizeClass::Medium);
        assert_eq!(at_medium.get("gap"), Some(&&1));
        assert_eq!(at_medium.get("padding"), None);
        let at_large = responsive.resolve(SizeClass::Large);
        assert_eq!(at_large.get("gap"), Some(&&2));
        assert_eq!(at_large.get("fill"), Some(&&1));
        assert_eq!(at_large.get("padding"), Some(&&9));
        assert!(responsive.resolve(SizeClass::Compact).is_empty());
    }
}
