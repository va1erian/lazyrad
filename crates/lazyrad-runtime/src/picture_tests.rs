//! A real project driving a `PictureBox` from its script: the bytes come from
//! `file_read_bytes` (a project asset), the control is called through the Rhai
//! bridge (`Value::Bytes` both ways) and its events reach the script.

use std::fs;
use std::path::PathBuf;

use xui_core::image::Image;
use xui_form::Value;

use crate::testing::{TestApp, run_on_large_stack};

const PROJECT: &str = r#"
name = "viewer"
version = "0.1.0"
startup = "main_form"
assets = ["pictures/*.png"]

[[items]]
kind = "form"
name = "main_form"
layout = "main_form.lfm"
code = "main_form.rhai"
"#;

const LAYOUT: &str = r#"
format = 1

[window]
name = "main_form"
title = "Viewer"
width = 320
height = 240

[[node]]
kind = "PictureBox"
name = "picture1"
left = 0
top = 0
width = 320
height = 200

[[node]]
kind = "Label"
name = "info"
left = 0
top = 210
width = 320
height = 20
"#;

const SCRIPT: &str = r#"
fn form_load() {
    picture1.load(file_read_bytes("pictures/wide.png"));
    show();
}

fn show() {
    info.text = `${picture1.image_width}x${picture1.image_height} ${picture1.format} ${picture1.zoom}% ${picture1.rotation}`;
}

fn picture1_key_down(key) {
    if key == "right" { picture1.rotate_cw(); }
    if key == "add" { picture1.zoom_in(); }
    show();
}

fn picture1_wheel(delta, ctrl) {
    if ctrl { if delta > 0 { picture1.zoom_in(); } else { picture1.zoom_out(); } }
    show();
}

fn picture1_double_click(x, y) {
    let png = picture1.to_png();
    file_write_bytes(app.path + "/rotated.png", png);
    info.text = `saved ${png.len()}`;
}

fn picture1_click(x, y) {
    try {
        picture1.load(blob(4, 0));
    } catch (error) {
        info.text = "refused";
    }
}
"#;

/// A scratch project folder with a 640x100 picture as its asset.
fn project_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lazyrad-picture-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("pictures")).expect("project folder");
    fs::write(dir.join("viewer.lrp"), PROJECT).expect("project");
    fs::write(dir.join("main_form.lfm"), LAYOUT).expect("layout");
    fs::write(dir.join("main_form.rhai"), SCRIPT).expect("script");
    let image = Image::from_rgba(640, 100, vec![0x40; 640 * 100 * 4]).expect("image");
    fs::write(
        dir.join("pictures/wide.png"),
        image.encode_png().expect("png"),
    )
    .expect("asset");
    dir
}

#[test]
fn a_script_loads_zooms_rotates_and_saves_a_picture() {
    let dir = project_dir();
    let saved = dir.join("rotated.png");
    run_on_large_stack(move || {
        TestApp::run(&dir, |app| {
            // Fitted: 640 wide into 320 is 50%.
            assert_eq!(app.text("info"), "640x100 PNG 50.0% 0");
            app.event("picture1", "KeyDown", vec![Value::Text("right".into())]);
            assert_eq!(
                app.text("info"),
                "640x100 PNG 31.25% 90",
                "fitted to the height once turned"
            );
            app.event(
                "picture1",
                "Wheel",
                vec![Value::Float(1.0), Value::Bool(true)],
            );
            assert_eq!(app.text("info"), "640x100 PNG 33.0% 90");
            app.event(
                "picture1",
                "DoubleClick",
                vec![Value::Float(1.0), Value::Float(1.0)],
            );
            assert!(
                app.text("info").starts_with("saved "),
                "{}",
                app.text("info")
            );
            app.event(
                "picture1",
                "Click",
                vec![Value::Float(1.0), Value::Float(1.0)],
            );
            assert_eq!(
                app.text("info"),
                "refused",
                "a bad file is a catchable error"
            );
            assert_eq!(app.get("picture1", "image_width"), Some(Value::Int(640)));
        })
        .expect("the project runs");
    });
    let rotated = Image::decode(&fs::read(&saved).expect("saved")).expect("a PNG");
    assert_eq!(rotated.size(), (100, 640), "saved as shown, rotated");
}
