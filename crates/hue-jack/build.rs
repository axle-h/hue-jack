//! Picks the web UI directory to embed: `web/dist` once it has been built, otherwise `web/placeholder`.
use std::path::Path;

fn main() {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
    let dist = web.join("dist");
    let dir = if dist.join("index.html").exists() {
        dist
    } else {
        web.join("placeholder")
    };
    let dir = dir.canonicalize().expect("web UI directory exists");
    println!("cargo:rustc-env=HUEJACK_WEB_DIR={}", dir.display());
    println!("cargo:rerun-if-changed={}", web.join("dist").display());
    println!(
        "cargo:rerun-if-changed={}",
        web.join("placeholder").display()
    );
}
