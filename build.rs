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
        .with_config(cbindgen::Config::from_root_or_default(PathBuf::from(
            "cbindgen.toml",
        )))
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(&output_file);

    let mut header =
        std::fs::read_to_string(&output_file).expect("Unable to read generated header");

    if !header.contains("#define QEM_DEPRECATED(msg)") {
        let marker = "using LogCallback = void(*)(const char*);\n";
        let deprecate_block = "\n#if defined(_MSC_VER)\n#define QEM_DEPRECATED(msg) __declspec(deprecated(msg))\n#elif defined(__GNUC__) || defined(__clang__)\n#define QEM_DEPRECATED(msg) __attribute__((deprecated(msg)))\n#else\n#define QEM_DEPRECATED(msg)\n#endif\n";
        if let Some(pos) = header.find(marker) {
            let insert_pos = pos + marker.len();
            header.insert_str(insert_pos, deprecate_block);
        }
    }

    header = header.replace(
        "float simplify_mesh(",
        "QEM_DEPRECATED(\"Use qem_simplify (ABI v2). Legacy ABI is for internal correctness testing only.\")\nfloat simplify_mesh(",
    );

    header = header.replace(
        "void get_last_simplify_result(",
        "QEM_DEPRECATED(\"Use qem_get_last_result (ABI v2). Legacy ABI is for internal correctness testing only.\")\nvoid get_last_simplify_result(",
    );

    std::fs::write(&output_file, header).expect("Unable to write patched header");
}
