//! The `examples/brickbreaker` sample: it checks clean, and it plays.
//!
//! The play test builds the sample's form on the offscreen backend through the
//! snapshot harness, which sends messages and input to the live app: a few
//! `Frame` events with the ball resting on the paddle, a real Space key press
//! (the form gives the canvas the focus in `form_load`), then more frames. The
//! ball is found on the rendered pixels by its colour, so the test sees what a
//! player would. The last frame is saved as `target/brickbreaker.png`.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_runtime::check::check_project;
use lazyrad_runtime::form::{FormRuntime, Msg};
use xui_canvas::snapshot::{Snapshot, render_with};
use xui_core::backend::Event;
use xui_core::image::Image;
use xui_core::message::{Key, Modifiers};
use xui_core::units::Dip;
use xui_form::Value;

/// The sample's directory.
fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/brickbreaker")
}

/// The ball's colour in the sample (`#ffe066`), used by nothing else.
const BALL: [u8; 3] = [0xff, 0xe0, 0x66];

/// A `Frame` event for the sample's canvas, `dt` seconds after the last.
fn frame(dt: f64) -> Msg {
    Msg::Event {
        form: "main_form".to_owned(),
        control: "game_canvas".to_owned(),
        event: "Frame".to_owned(),
        args: vec![Value::Float(dt)],
    }
}

/// The centre of the pixels painted exactly in the ball's colour.
fn ball_centre(image: &Image) -> Option<(f64, f64)> {
    let (mut sum_x, mut sum_y, mut count) = (0.0, 0.0, 0.0);
    for y in 0..image.height() {
        for x in 0..image.width() {
            if image
                .pixel(x, y)
                .is_some_and(|[r, g, b, _]| [r, g, b] == BALL)
            {
                sum_x += f64::from(x);
                sum_y += f64::from(y);
                count += 1.0;
            }
        }
    }
    (count > 0.0).then(|| (sum_x / count, sum_y / count))
}

/// Whether any pixel has exactly the colour `rgb`.
fn has_colour(image: &Image, rgb: [u8; 3]) -> bool {
    (0..image.height()).any(|y| {
        (0..image.width()).any(|x| {
            image
                .pixel(x, y)
                .is_some_and(|[r, g, b, _]| [r, g, b] == rgb)
        })
    })
}

#[test]
fn the_sample_checks_clean() {
    let report = check_project(sample_dir()).expect("the sample loads");
    assert!(report.is_empty(), "{report:?}");
    let project = lazyrad_project::Project::load(&sample_dir()).expect("the project loads");
    assert!(project.validate(&sample_dir()).is_empty());
}

#[test]
fn space_launches_the_ball_and_frames_move_it() {
    let runtime = FormRuntime::load(sample_dir()).expect("the sample project loads");
    // Every handler that succeeds is counted; one that fails is not (it
    // opens an error box instead).
    let handled = Rc::new(Cell::new(0));
    let counter = Rc::clone(&handled);
    runtime.set_handler_observer(Rc::new(move |_, _, _| counter.set(counter.get() + 1)));

    let before: Rc<RefCell<Option<Image>>> = Rc::new(RefCell::new(None));
    let before_slot = Rc::clone(&before);
    let image = render_with(
        Snapshot::new(Dip(640.0), Dip(520.0)).title("Brick Breaker"),
        move |ui| Ok(runtime.build_app(ui, "main_form").expect("the form builds")),
        move |stage| {
            for _ in 0..3 {
                stage.emit(frame(1.0 / 60.0));
            }
            *before_slot.borrow_mut() = Some(stage.ui().capture().expect("a capture"));
            // A real key press: `form_load` focused the canvas.
            let consumed = stage.inject(Event::KeyDown {
                key: Key::SPACE,
                modifiers: Modifiers::NONE,
                repeat: 1,
                system: false,
            });
            assert!(consumed, "the canvas has the focus and reports Space");
            // About 1.2 s: up through the gap, into the bottom row and back.
            for _ in 0..70 {
                stage.emit(frame(1.0 / 60.0));
            }
        },
    )
    .expect("the sample renders");

    // 3 frames, the Space key and 70 frames, all without an error.
    assert_eq!(handled.get(), 74, "a handler failed");

    let before = before.borrow_mut().take().expect("the first capture");
    let (x0, y0) = ball_centre(&before).expect("the ball is drawn on the paddle");
    let (x1, y1) = ball_centre(&image).expect("the ball is drawn in play");
    // Served at about 300 px/s, it reaches the bricks 270 px up and falls
    // back a little.
    assert!(y1 < y0 - 60.0, "the ball rose from {y0} to {y1}");
    assert!((x1 - x0).abs() < 120.0, "served roughly straight up");
    // A brick of the bottom row is gone: the background shows where it was.
    let gaps = (16..624)
        .filter(|x| {
            before
                .pixel(*x, 194)
                .is_some_and(|[r, g, b, _]| [r, g, b] == [155, 89, 182])
                && image
                    .pixel(*x, 194)
                    .is_some_and(|[r, g, b, _]| [r, g, b] == [0x10, 0x10, 0x18])
        })
        .count();
    assert!(gaps > 20, "the ball broke a brick ({gaps} pixels cleared)");
    // The wall: the bottom (purple) row is drawn at full strength.
    assert!(has_colour(&image, [155, 89, 182]), "the bricks are drawn");
    // The score bar sits above everything.
    assert!(has_colour(&image, [0x1c, 0x1c, 0x2c]));

    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::create_dir_all(&target).expect("the target directory exists");
    image
        .save_png(target.join("brickbreaker.png"))
        .expect("the screenshot is written");
}
