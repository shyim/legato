use std::fs;
use std::io;
use std::path::Path;

use mago_prelude::Prelude;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo");
    let prelude_file = Path::new(&out_dir).join("prelude.bin");
    let encoded = std::thread::Builder::new()
        .name("legato-prelude".to_owned())
        .stack_size(36 * 1024 * 1024)
        .spawn(|| Prelude::build().encode().expect("Mago prelude can be encoded"))?
        .join()
        .expect("Mago prelude builder did not panic");

    fs::write(prelude_file, encoded)
}
