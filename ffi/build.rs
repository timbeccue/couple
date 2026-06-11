fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = std::path::Path::new(&crate_dir).join("couple.h");
    match cbindgen::generate(&crate_dir) {
        Ok(bindings) => {
            bindings.write_to_file(&out);
        }
        Err(e) => {
            // Header generation failures must not break normal builds; CI diffs the
            // committed header instead.
            println!("cargo:warning=cbindgen failed: {e}");
        }
    }
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
