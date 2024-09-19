fn main() {
    bindgen_minimp3();
}

fn bindgen_minimp3() {
    let bindings = bindgen::Builder::default()
        .header("../minimp3/minimp3.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    bindings
        .write_to_file("./src/minimp3_bindings.rs")
        .expect("Couldn't write bindings!");

    cc::Build::new()
        .file("./minimp3.c")
        .include("../minimp3")
        .compile("minimp3");
}
