//! The timing curves, against the literals and properties of
//! `xui_core::motion`'s easing (this crate re-implements them and must stay
//! byte-compatible with them).

use lazyrad_view::{DEFAULT_EXPONENT, Easing};

/// Every curve, with exponential curves at several exponents (positive, steep,
/// shallow and negative).
fn curves() -> Vec<Easing> {
    let mut all = vec![
        Easing::Linear,
        Easing::QuadIn,
        Easing::QuadOut,
        Easing::QuadInOut,
        Easing::CubicIn,
        Easing::CubicOut,
        Easing::CubicInOut,
        Easing::expo_in(),
        Easing::expo_out(),
    ];
    for exponent in [
        0.01,
        1.0,
        2.0,
        12.0,
        40.0,
        300.0,
        -3.0,
        f32::MAX,
        -f32::MAX,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        all.push(Easing::ExpoIn { exponent });
        all.push(Easing::ExpoOut { exponent });
    }
    all
}

/// The textbook ease-in, `(e^(k t) - 1) / (e^k - 1)`, in `f64`.
fn closed_form_in(k: f64, t: f64) -> f64 {
    ((k * t).exp() - 1.0) / (k.exp() - 1.0)
}

/// The textbook ease-out, `1 - f(1 - t)`, in `f64`.
fn closed_form_out(k: f64, t: f64) -> f64 {
    1.0 - closed_form_in(k, 1.0 - t)
}

#[test]
fn every_curve_starts_at_exactly_zero_and_ends_at_exactly_one() {
    for easing in curves() {
        assert_eq!(easing.ease(0.0), 0.0, "{easing:?}");
        assert_eq!(easing.ease(1.0), 1.0, "{easing:?}");
    }
}

#[test]
fn the_default_exponent_hits_the_known_literals() {
    // (e^3 - 1) / (e^6 - 1) = 0.047426, and its mirror 0.952574.
    assert_eq!(DEFAULT_EXPONENT, 6.0);
    let ease_in = Easing::expo_in().ease(0.5);
    let ease_out = Easing::expo_out().ease(0.5);
    assert!((ease_in - 0.04743).abs() < 1.0e-5, "{ease_in}");
    assert!((ease_out - 0.95257).abs() < 1.0e-5, "{ease_out}");
    // The bits, which is what "byte-compatible" means: these are what xui's
    // `Easing::ease` returns for the same input.
    assert_eq!(ease_in.to_bits(), 0.047_425_874_f32.to_bits());
    assert_eq!(ease_out.to_bits(), 0.952_574_13_f32.to_bits());
}

#[test]
fn every_curve_is_monotonic_and_stays_in_the_unit_range_over_a_sweep() {
    for easing in curves() {
        let mut previous = 0.0_f32;
        for i in 0..=1000 {
            let value = easing.ease(i as f32 / 1000.0);
            assert!((0.0..=1.0).contains(&value), "{easing:?} at {i}: {value}");
            assert!(
                value >= previous,
                "{easing:?} went backwards at {i}: {previous} -> {value}"
            );
            previous = value;
        }
        assert_eq!(previous, 1.0, "{easing:?} ends at 1.0");
    }
}

#[test]
fn polynomial_curves_hit_their_known_values() {
    assert_eq!(Easing::Linear.ease(0.25), 0.25);
    assert_eq!(Easing::QuadIn.ease(0.5), 0.25);
    assert_eq!(Easing::QuadOut.ease(0.5), 0.75);
    assert_eq!(Easing::CubicIn.ease(0.5), 0.125);
    assert_eq!(Easing::CubicOut.ease(0.5), 0.875);
    assert_eq!(Easing::QuadInOut.ease(0.5), 0.5);
    assert_eq!(Easing::CubicInOut.ease(0.5), 0.5);
    assert_eq!(Easing::QuadInOut.ease(0.25), 0.125);
    assert_eq!(Easing::QuadInOut.ease(0.75), 0.875);
    assert_eq!(Easing::CubicInOut.ease(0.25), 0.0625);
    assert_eq!(Easing::CubicInOut.ease(0.75), 0.9375);
}

#[test]
fn exponential_curves_match_the_closed_form() {
    let out = Easing::ExpoOut {
        exponent: DEFAULT_EXPONENT,
    };
    for t in [0.1_f32, 0.5, 0.9] {
        let expected = closed_form_out(6.0, f64::from(t));
        assert!(
            (f64::from(out.ease(t)) - expected).abs() < 1.0e-6,
            "t = {t}"
        );
    }
    // The fast start and the slow settle that give Metro its feel.
    assert!(out.ease(0.4) > 0.9);
    assert!(out.ease(0.1) > 0.4);
    for k in [0.5_f32, 2.0, 6.0, 20.0] {
        let ease_in = Easing::ExpoIn { exponent: k };
        let ease_out = Easing::ExpoOut { exponent: k };
        for t in [0.05_f32, 0.25, 0.5, 0.75, 0.95] {
            let expected = closed_form_in(f64::from(k), f64::from(t));
            assert!((f64::from(ease_in.ease(t)) - expected).abs() < 1.0e-6);
        }
        // The two mirror each other, and a negative exponent swaps them.
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            let mirrored = 1.0 - ease_in.ease(1.0 - t);
            assert!(
                (ease_out.ease(t) - mirrored).abs() < 1.0e-6,
                "k = {k}, t = {t}"
            );
            let negated = Easing::ExpoIn { exponent: -k }.ease(t);
            assert!(
                (negated - ease_out.ease(t)).abs() < 1.0e-6,
                "k = {k}, t = {t}"
            );
        }
    }
}

#[test]
fn time_outside_the_unit_interval_clamps_and_nan_is_the_start() {
    for easing in curves() {
        for below in [-0.001, -1.0, -1.0e30, f32::NEG_INFINITY] {
            assert_eq!(easing.ease(below), 0.0, "{easing:?} at {below}");
        }
        for above in [1.001, 2.0, 1.0e30, f32::INFINITY] {
            assert_eq!(easing.ease(above), 1.0, "{easing:?} at {above}");
        }
        assert_eq!(easing.ease(f32::NAN), 0.0, "{easing:?}");
        assert!(easing.ease(-0.0).is_sign_positive(), "{easing:?}");
    }
}

#[test]
fn degenerate_exponents_are_defined() {
    // Zero is the limit of the family and NaN is not rejected: both linear.
    for exponent in [0.0, -0.0, f32::NAN] {
        for t in [0.1_f32, 0.5, 0.9] {
            assert_eq!(Easing::ExpoIn { exponent }.ease(t), t, "in {exponent}");
            assert_eq!(Easing::ExpoOut { exponent }.ease(t), t, "out {exponent}");
        }
    }
    // An infinite or huge exponent is a step with no NaN on the way.
    for exponent in [f32::INFINITY, f32::MAX] {
        assert_eq!(Easing::ExpoIn { exponent }.ease(0.999), 0.0);
        assert_eq!(Easing::ExpoOut { exponent }.ease(0.001), 1.0);
    }
    for exponent in [f32::NEG_INFINITY, -f32::MAX] {
        for t in [1.0e-6, 0.5, 0.999_999] {
            assert_eq!(Easing::ExpoIn { exponent }.ease(t), 1.0);
            assert_eq!(Easing::ExpoOut { exponent }.ease(t), 0.0);
        }
    }
}

#[test]
fn the_trace_names_are_stable_and_nan_exponents_compare_equal() {
    let names: Vec<String> = [
        Easing::Linear,
        Easing::QuadIn,
        Easing::QuadOut,
        Easing::QuadInOut,
        Easing::CubicIn,
        Easing::CubicOut,
        Easing::CubicInOut,
        Easing::expo_in(),
        Easing::ExpoOut { exponent: 2.5 },
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        names,
        [
            "linear",
            "quad_in",
            "quad_out",
            "quad_in_out",
            "cubic_in",
            "cubic_out",
            "cubic_in_out",
            "expo_in(6.0)",
            "expo_out(2.5)",
        ]
    );
    let nan = Easing::ExpoOut { exponent: f32::NAN };
    assert_eq!(nan, nan);
    assert_ne!(nan, Easing::ExpoIn { exponent: f32::NAN });
    assert_ne!(Easing::expo_out(), Easing::ExpoOut { exponent: 5.0 });
}
