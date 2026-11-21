//! Build-time configuration for the derive macros.

include!("build_naming.rs");

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    emit_naming_cfgs();
}
