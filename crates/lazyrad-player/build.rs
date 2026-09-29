//! Embeds LazyRAD's icon into the Windows `lazyrad-player` executable.
//!
//! Export copies the player, so this is the default icon of every exported app
//! whose project sets none. Only the binary target gets the resource, and only
//! when building for Windows; elsewhere this does nothing.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=lazyrad-player.rc");
        println!("cargo:rerun-if-changed=../../assets/rye-shades.ico");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            embed_resource::compile_for(
                "lazyrad-player.rc",
                ["lazyrad-player"],
                embed_resource::NONE,
            )
            .manifest_required()
            .expect("the application icon resource compiles");
        }
    }
}
