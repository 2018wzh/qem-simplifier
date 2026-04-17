extern crate cbindgen;

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let crate_dir_path = PathBuf::from(&crate_dir);
    let package_name = env::var("CARGO_PKG_NAME").unwrap();
    let output_file = crate_dir_path
        .join("include")
        .join(format!("{}.h", package_name.replace("-", "_")));

    // 确保 include 目录存在
    std::fs::create_dir_all(output_file.parent().unwrap()).unwrap();

    cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(cbindgen::Config::from_root_or_default(PathBuf::from(
            "cbindgen.toml",
        )))
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(&output_file);

    let plugin_root = crate_dir_path.join("plugins").join("QEMMeshReduction");
    if plugin_root.exists() {
        let plugin_header = plugin_root
            .join("Source")
            .join("ThirdParty")
            .join("QEMSimplifier")
            .join("Include")
            .join(format!("{}.h", package_name.replace("-", "_")));

        if let Some(parent) = plugin_header.parent() {
            std::fs::create_dir_all(parent).expect("Unable to create plugin include directory");
        }

        std::fs::copy(&output_file, &plugin_header)
            .expect("Unable to sync generated header to plugin include path");
    }
}
