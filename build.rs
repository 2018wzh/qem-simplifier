extern crate cbindgen;

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let package_name = env::var("CARGO_PKG_NAME").unwrap();
    let output_file = PathBuf::from(&crate_dir)
        .join("include")
        .join(format!("{}.h", package_name.replace("-", "_")));

    // 确保 include 目录存在
    std::fs::create_dir_all(output_file.parent().unwrap()).unwrap();

    cbindgen::Builder::new()
      .with_crate(crate_dir)
      .with_config(cbindgen::Config::from_root_or_default(PathBuf::from("cbindgen.toml")))
      .generate()
      .expect("Unable to generate bindings")
      .write_to_file(output_file);
}
