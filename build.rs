use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let catalog_dir = manifest_dir.join("catalog");
    println!("cargo:rerun-if-changed=catalog");

    let mut entries: Vec<(String, PathBuf)> = fs::read_dir(&catalog_dir)
        .unwrap_or_else(|e| panic!("cannot read catalog directory {}: {e}", catalog_dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("non-UTF8 catalog filename")
                .to_string();
            (name, path)
        })
        .collect();

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let dest_path = out_dir.join("embedded_catalog.rs");
    let mut out = fs::File::create(&dest_path)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", dest_path.display()));

    writeln!(out, "pub const EMBEDDED_CATALOG: &[(&str, &str)] = &[").unwrap();
    for (name, path) in &entries {
        println!("cargo:rerun-if-changed=catalog/{name}");
        let contents =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        writeln!(out, "    ({name:?}, {contents:?}),").unwrap();
    }
    writeln!(out, "];").unwrap();
}
