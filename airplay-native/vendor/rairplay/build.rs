fn main() {
    let mut build = cc::Build::new();
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("shairplay/src/lib/playfair");
    for name in [
        "playfair.c",
        "hand_garble.c",
        "modified_md5.c",
        "omg_hax.c",
        "sap_hash.c",
    ] {
        let path = root.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        build.file(path);
    }
    println!(
        "cargo:rerun-if-changed={}",
        root.join("playfair.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("omg_hax.h").display()
    );
    build.cargo_warnings(false).compile("fairplay3");
}
