//! Embeds the Geist fonts (SIL OFL 1.1) from `assets/fonts/` when they are
//! there. When they are missing the report falls back to system fonts and
//! the build still works.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let fonts = manifest.join("..").join("..").join("assets").join("fonts");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("fonts.rs");
    let mut code = String::new();
    for (name, file) in [
        ("GEIST", "Geist-Variable.woff2"),
        ("GEIST_MONO", "GeistMono-Variable.woff2"),
    ] {
        let path = fonts.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        if path.exists() {
            let shown = path.display().to_string();
            code.push_str(&format!(
                "pub const {name}: Option<&[u8]> = Some(include_bytes!({shown:?}));\n"
            ));
        } else {
            println!(
                "cargo:warning=promochaos-report: {file} not found; report.html uses system fonts"
            );
            code.push_str(&format!("pub const {name}: Option<&[u8]> = None;\n"));
        }
    }
    if let Err(e) = std::fs::write(&out, code) {
        println!(
            "cargo:warning=promochaos-report: could not write {}: {e}",
            out.display()
        );
    }
}
