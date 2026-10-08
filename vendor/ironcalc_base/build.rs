// The vendored copy has no IronCalc checkout to ask `git describe`, and asking the
// enclosing repository would report Zenkai's version, so INFO("release") reports the
// crate version.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=GIT_VERSION=v{}", env!("CARGO_PKG_VERSION"));
}
