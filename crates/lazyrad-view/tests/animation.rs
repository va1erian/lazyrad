//! Animations as data: sampling, the virtual clock, and the patch that carries
//! one.

use lazyrad_view::limits::MAX_ANIMATION_MS;
use lazyrad_view::{
    AnimFrom, Animation, AnimationSampler, Easing, Patch, RecordingRenderer, Renderer,
    SamplingRenderer, Value, trace,
};

fn animate(node: &str, property: &str, from: AnimFrom, to: f64) -> Patch {
    Patch::Animate {
        node: node.into(),
        property: property.into(),
        from,
        to,
        duration_ms: 100.0,
        delay_ms: 0.0,
        easing: Easing::Linear,
    }
}

#[test]
fn an_animation_holds_from_through_the_delay_and_ends_exactly_at_to() {
    let animation = Animation::new(10.0, 20.0, 100.0).with_delay(50.0);
    assert_eq!(animation.sample(0.0), 10.0);
    assert_eq!(animation.sample(49.0), 10.0);
    assert_eq!(animation.sample(50.0), 10.0);
    assert_eq!(animation.sample(100.0), 15.0);
    assert!(animation.sample(149.0) < 20.0);
    assert_eq!(animation.sample(150.0), 20.0);
    assert_eq!(animation.sample(1.0e9), 20.0);
    assert!(!animation.is_finished(149.0));
    assert!(animation.is_finished(150.0));
    assert_eq!(animation.end_ms(), 150.0);
}

#[test]
fn the_curve_shapes_the_value_and_the_value_stays_between_the_endpoints() {
    let animation = Animation::new(40.0, 0.0, 350.0).with_easing(Easing::expo_out());
    let at = |ms: f64| animation.sample(ms);
    // The golden numbers of the plan's entrance, computed from the closed form.
    assert!((at(100.0) - 7.122_197).abs() < 1.0e-5, "{}", at(100.0));
    assert!((at(250.0) - 0.452_523).abs() < 1.0e-5, "{}", at(250.0));
    let mut previous = 40.0;
    for step in 0..=350 {
        let value = at(f64::from(step));
        assert!((0.0..=40.0).contains(&value), "{step}: {value}");
        assert!(value <= previous, "{step}: {previous} -> {value}");
        previous = value;
    }
    // Endpoints are returned as given, also when either is negative.
    let negative = Animation::new(-1.5, -9.25, 10.0).with_easing(Easing::CubicOut);
    assert_eq!(negative.sample(0.0), -1.5);
    assert_eq!(negative.sample(10.0), -9.25);
}

#[test]
fn nan_negative_and_huge_numbers_have_a_defined_meaning() {
    // A duration that is NaN or negative is zero: `from` until the delay, then
    // `to`; it is finished at once.
    for duration in [f64::NAN, -5.0, f64::NEG_INFINITY, 0.0] {
        let animation = Animation::new(1.0, 2.0, duration).with_delay(10.0);
        assert_eq!(animation.sample(9.0), 1.0, "{duration}");
        assert_eq!(animation.sample(10.0), 2.0, "{duration}");
        assert!(animation.is_finished(10.0));
        assert_eq!(animation.end_ms(), 10.0);
    }
    // A delay that is NaN or negative is zero.
    for delay in [f64::NAN, -1.0] {
        let animation = Animation::new(1.0, 2.0, 100.0).with_delay(delay);
        assert_eq!(animation.sample(0.0), 1.0);
        assert_eq!(animation.sample(50.0), 1.5);
    }
    // Anything above the limit is cut to it, so an infinite one still ends.
    let long = Animation::new(0.0, 1.0, f64::INFINITY);
    assert_eq!(long.end_ms(), MAX_ANIMATION_MS);
    assert_eq!(long.sample(MAX_ANIMATION_MS), 1.0);
    assert!(long.is_finished(MAX_ANIMATION_MS));
    // Time that is NaN is the start; time before it is before it.
    let animation = Animation::new(3.0, 4.0, 100.0);
    assert_eq!(animation.sample(f64::NAN), 3.0);
    assert_eq!(animation.sample(-50.0), 3.0);
    assert_eq!(animation.sample(f64::INFINITY), 4.0);
    assert!(!animation.is_finished(f64::NAN));
    assert!(Animation::new(3.0, 4.0, 0.0).is_finished(f64::NAN));
    // Endpoints that are not finite are held, then returned exactly.
    let infinite = Animation::new(f64::NEG_INFINITY, 1.0, 100.0);
    assert_eq!(infinite.sample(50.0), f64::NEG_INFINITY);
    assert_eq!(infinite.sample(100.0), 1.0);
    let nan = Animation::new(0.0, f64::NAN, 100.0);
    assert_eq!(nan.sample(50.0), 0.0);
    assert!(nan.sample(100.0).is_nan());
    // Endpoints of opposite sign near the limit do not overflow mid-way.
    let wide = Animation::new(f64::MAX, -f64::MAX, 100.0);
    assert!(wide.sample(50.0).is_finite());
}

#[test]
fn the_sampler_runs_on_a_clock_that_only_moves_when_told() {
    let mut sampler = AnimationSampler::new();
    assert!(sampler.is_empty());
    sampler.start("card", "opacity", Animation::new(0.0, 1.0, 100.0));
    sampler.start("card", "translate_y", Animation::new(40.0, 0.0, 200.0));
    assert_eq!(sampler.len(), 2);
    assert_eq!(sampler.value("card", "opacity"), Some(0.0));

    let frame = sampler.advance(0.0);
    assert_eq!(frame.time_ms, 0.0);
    assert_eq!(frame.value("card", "opacity"), Some(0.0));
    assert!(frame.finished.is_empty());

    let frame = sampler.advance(50.0);
    assert_eq!(frame.value("card", "opacity"), Some(0.5));
    assert_eq!(frame.value("card", "translate_y"), Some(30.0));
    assert_eq!(sampler.now_ms(), 50.0);

    // The first one ends here, with its exact final value, and is gone.
    let frame = sampler.advance(50.0);
    assert_eq!(frame.value("card", "opacity"), Some(1.0));
    assert_eq!(frame.finished, [("card".to_owned(), "opacity".to_owned())]);
    assert_eq!(sampler.len(), 1);
    assert_eq!(sampler.value("card", "opacity"), None);

    // The clock never goes backwards.
    for dt in [-10.0, f64::NAN] {
        let before = sampler.now_ms();
        let frame = sampler.advance(dt);
        assert_eq!(frame.time_ms, before);
    }
    let frame = sampler.advance(100.0);
    assert_eq!(
        frame.finished,
        [("card".to_owned(), "translate_y".to_owned())]
    );
    assert!(sampler.is_empty());
}

#[test]
fn starting_replaces_and_cancelling_stops_without_finishing() {
    let mut sampler = AnimationSampler::new();
    sampler.start("a", "opacity", Animation::new(0.0, 1.0, 100.0));
    sampler.advance(50.0);
    // A newer animation on the same key replaces it, starting from the clock.
    sampler.start("a", "opacity", Animation::new(0.5, 0.0, 100.0));
    assert_eq!(sampler.len(), 1);
    let frame = sampler.advance(50.0);
    assert_eq!(frame.value("a", "opacity"), Some(0.25));
    assert!(sampler.cancel("a", "opacity"));
    assert!(!sampler.cancel("a", "opacity"));
    assert!(sampler.advance(500.0).finished.is_empty());
    sampler.start("b", "opacity", Animation::new(0.0, 1.0, 10.0));
    sampler.start("b", "rotate", Animation::new(0.0, 1.0, 10.0));
    sampler.start("c", "rotate", Animation::new(0.0, 1.0, 10.0));
    sampler.cancel_node("b");
    assert_eq!(sampler.len(), 1);
    sampler.clear();
    assert!(sampler.is_empty());
}

#[test]
fn the_sampling_renderer_animates_resolves_current_and_reports_the_end() {
    let mut renderer = SamplingRenderer::new();
    renderer.apply(&[
        Patch::Create {
            node: "card".into(),
            parent: None,
            index: 0,
            kind: "Panel".into(),
            props: [("opacity".to_owned(), Value::Float(1.0))].into(),
        },
        animate("card", "opacity", AnimFrom::Value(0.0), 1.0),
    ]);
    assert_eq!(renderer.running(), 1);
    // The widget shows the animated value, not the target.
    assert_eq!(renderer.value("card", "opacity"), Some(0.0));
    assert_eq!(
        renderer.live_value("card", "opacity"),
        Some(Value::Float(0.0))
    );
    let frame = renderer.advance(50.0);
    assert_eq!(frame.value("card", "opacity"), Some(0.5));

    // An animation from `Current` starts where the widget is.
    renderer.apply(&[animate("card", "opacity", AnimFrom::Current, 0.0)]);
    assert_eq!(renderer.value("card", "opacity"), Some(0.5));
    let frame = renderer.advance(50.0);
    assert_eq!(frame.value("card", "opacity"), Some(0.25));
    let frame = renderer.advance(50.0);
    assert_eq!(frame.finished, [("card".to_owned(), "opacity".to_owned())]);
    // Afterwards the live value is the end value.
    assert_eq!(renderer.value("card", "opacity"), Some(0.0));
    assert_eq!(renderer.running(), 0);

    // A `Set` cancels an animation in flight, unreported.
    renderer.apply(&[animate("card", "opacity", AnimFrom::Value(0.0), 1.0)]);
    renderer.apply(&[Patch::Set {
        node: "card".into(),
        property: "opacity".into(),
        value: Value::Float(0.3),
    }]);
    assert_eq!(renderer.running(), 0);
    assert_eq!(renderer.value("card", "opacity"), Some(0.3));
    assert!(renderer.advance(500.0).finished.is_empty());
}

/// `from: Current` for a property the widget has no number for starts at `to`:
/// there is nothing to animate.
#[test]
fn current_without_a_known_value_starts_at_the_target() {
    let mut renderer = SamplingRenderer::new();
    renderer.apply(&[animate("ghost", "opacity", AnimFrom::Current, 0.8)]);
    assert_eq!(renderer.value("ghost", "opacity"), Some(0.8));
    let frame = renderer.advance(50.0);
    assert_eq!(frame.value("ghost", "opacity"), Some(0.8));
    let frame = renderer.advance(50.0);
    assert_eq!(frame.finished, [("ghost".to_owned(), "opacity".to_owned())]);
}

#[test]
fn the_trace_line_of_an_animate_is_stable() {
    let enter = Patch::Animate {
        node: "counter_card".into(),
        property: "translate_y".into(),
        from: AnimFrom::Value(40.0),
        to: 0.0,
        duration_ms: 350.0,
        delay_ms: 0.0,
        easing: Easing::expo_out(),
    };
    assert_eq!(
        enter.to_string(),
        "animate counter_card.translate_y from=40.0 to=0.0 duration=350.0 delay=0.0 \
         easing=expo_out(6.0)"
    );
    let release = Patch::Animate {
        node: "add_button".into(),
        property: "scale_x".into(),
        from: AnimFrom::Current,
        to: 1.0,
        duration_ms: 100.0,
        delay_ms: 200.0,
        easing: Easing::CubicOut,
    };
    assert_eq!(
        trace(&[release]),
        "animate add_button.scale_x from=current to=1.0 duration=100.0 delay=200.0 \
         easing=cubic_out\n"
    );
    assert_eq!(enter.node(), "counter_card");
}

#[test]
fn patches_with_nan_compare_equal_to_themselves_and_not_to_others() {
    let nan = animate("a", "opacity", AnimFrom::Value(f64::NAN), f64::NAN);
    assert_eq!(nan, nan.clone());
    assert_ne!(nan, animate("a", "opacity", AnimFrom::Value(0.0), f64::NAN));
    assert_ne!(nan, animate("a", "opacity", AnimFrom::Current, f64::NAN));
    assert_ne!(
        animate("a", "opacity", AnimFrom::Current, 1.0),
        animate("b", "opacity", AnimFrom::Current, 1.0)
    );
    // Other variants still compare field by field.
    let set = Patch::Set {
        node: "a".into(),
        property: "text".into(),
        value: Value::Text("x".into()),
    };
    assert_eq!(set, set.clone());
    assert_ne!(
        set,
        Patch::Unset {
            node: "a".into(),
            property: "text".into(),
        }
    );
}

#[test]
fn a_patch_describes_the_animation_a_renderer_samples() {
    let patch = animate("a", "opacity", AnimFrom::Current, 1.0);
    let from_current = patch.animation(Some(0.25)).unwrap();
    assert_eq!(from_current.from, 0.25);
    assert_eq!(from_current.to, 1.0);
    assert_eq!(patch.animation(None).unwrap().from, 1.0);
    let explicit = animate("a", "opacity", AnimFrom::Value(0.5), 1.0);
    assert_eq!(explicit.animation(Some(9.0)).unwrap().from, 0.5);
    let set = Patch::Remove { node: "a".into() };
    assert!(set.animation(None).is_none());
}

#[test]
fn a_recording_renderer_has_no_clock_and_applies_the_target_at_once() {
    let mut renderer = RecordingRenderer::new();
    renderer.apply(&[animate("a", "opacity", AnimFrom::Value(0.0), 0.7)]);
    assert_eq!(renderer.live_value("a", "opacity"), Some(Value::Float(0.7)));
}
