//! The environment a view is resolved against.

use rhai::{Dynamic, Map};

use crate::size_class::SizeClass;

/// What the host tells a view about the space it is drawn in.
///
/// There is deliberately **no platform field**: a binding that could ask
/// "am I on the web?" would invite the per-target branches the design forbids.
/// Everything a layout may depend on is a measurement or a preference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    /// Viewport width in dp (device-independent pixels).
    pub width: f32,
    /// Viewport height in dp.
    pub height: f32,
    /// Whether the dark theme is active.
    pub dark: bool,
    /// The user's text scale, `1.0` for the default size.
    pub text_scale: f32,
}

impl Environment {
    /// A light environment at the default text scale, `width` by `height` dp.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            dark: false,
            text_scale: 1.0,
        }
    }

    /// The size class of [`width`](Self::width).
    #[must_use]
    pub fn size_class(&self) -> SizeClass {
        SizeClass::from_width(self.width)
    }

    /// The read-only `env` map a binding sees:
    ///
    /// | Key          | Type   | Value                                  |
    /// |--------------|--------|----------------------------------------|
    /// | `width`      | float  | viewport width in dp                   |
    /// | `height`     | float  | viewport height in dp                  |
    /// | `size_class` | string | `"compact"`, `"medium"`, `"expanded"`, `"large"` or `"xlarge"` |
    /// | `dark`       | bool   | dark theme active                      |
    /// | `text_scale` | float  | text scale factor                      |
    ///
    /// Sizes are floats because dp are fractional; compare them with a float
    /// or an integer literal alike (`env.width >= 600`).
    #[must_use]
    pub fn to_map(&self) -> Map {
        let mut map = Map::new();
        map.insert("width".into(), Dynamic::from_float(f64::from(self.width)));
        map.insert("height".into(), Dynamic::from_float(f64::from(self.height)));
        map.insert(
            "size_class".into(),
            Dynamic::from(self.size_class().as_str().to_owned()),
        );
        map.insert("dark".into(), Dynamic::from(self.dark));
        map.insert(
            "text_scale".into(),
            Dynamic::from_float(f64::from(self.text_scale)),
        );
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_has_exactly_the_documented_keys_and_no_platform() {
        let env = Environment {
            width: 700.0,
            height: 500.0,
            dark: true,
            text_scale: 1.5,
        };
        let map = env.to_map();
        let keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
        assert_eq!(
            keys,
            ["dark", "height", "size_class", "text_scale", "width"]
        );
        assert!(!map.contains_key("platform"));
        assert_eq!(map["size_class"].clone().into_string().unwrap(), "medium");
        assert!(map["dark"].as_bool().unwrap());
        assert_eq!(map["width"].as_float().unwrap(), 700.0);
        assert_eq!(map["text_scale"].as_float().unwrap(), 1.5);
    }

    #[test]
    fn size_class_follows_the_width() {
        assert_eq!(
            Environment::new(360.0, 640.0).size_class(),
            SizeClass::Compact
        );
        assert_eq!(
            Environment::new(600.0, 640.0).size_class(),
            SizeClass::Medium
        );
        assert_eq!(
            Environment::new(f32::NAN, 640.0).size_class(),
            SizeClass::Compact
        );
        // A NaN width still produces a map instead of panicking.
        assert_eq!(
            Environment::new(f32::NAN, 1.0).to_map()["size_class"]
                .clone()
                .into_string()
                .unwrap(),
            "compact"
        );
    }
}
