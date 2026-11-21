use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn embed_toml_dir(manifest_dir: &Path, out_dir: &Path, dir_name: &str, out_file: &str, const_name: &str) {
    let src_dir = manifest_dir.join(dir_name);
    println!("cargo:rerun-if-changed={dir_name}");

    let mut entries: Vec<(String, PathBuf)> = fs::read_dir(&src_dir)
        .unwrap_or_else(|e| panic!("cannot read {dir_name} directory {}: {e}", src_dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| panic!("non-UTF8 {dir_name} filename"))
                .to_string();
            (name, path)
        })
        .collect();

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let dest_path = out_dir.join(out_file);
    let mut out = fs::File::create(&dest_path).unwrap_or_else(|e| panic!("cannot create {}: {e}", dest_path.display()));

    writeln!(out, "pub const {const_name}: &[(&str, &str)] = &[").unwrap();
    for (name, path) in &entries {
        println!("cargo:rerun-if-changed={dir_name}/{name}");
        let contents = fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        writeln!(out, "    ({name:?}, {contents:?}),").unwrap();
    }
    writeln!(out, "];").unwrap();
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));

    embed_toml_dir(&manifest_dir, &out_dir, "catalog", "embedded_catalog.rs", "EMBEDDED_CATALOG");
    embed_toml_dir(&manifest_dir, &out_dir, "lessons", "embedded_lessons.rs", "EMBEDDED_LESSONS");
    embed_toml_dir(&manifest_dir, &out_dir, "sprites", "embedded_sprites.rs", "EMBEDDED_SPRITES");
}
