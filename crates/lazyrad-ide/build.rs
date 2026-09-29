//! Embeds the application icon into the Windows `lazyrad-ide` executable.
//!
//! Only the binary target gets the resource, and only when building for
//! Windows. Elsewhere this does nothing (and `embed-resource` is not built
//! at all on a non-Windows host), so no resource compiler is needed.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=lazyrad-ide.rc");
        println!("cargo:rerun-if-changed=../../assets/rye-shades.ico");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            embed_resource::compile_for("lazyrad-ide.rc", ["lazyrad-ide"], embed_resource::NONE)
                .manifest_required()
                .expect("the application icon resource compiles");
        }
    }
}
