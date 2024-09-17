use std::process::Command;

fn main() {
    bindgen_minimp3();
    bindgen_libswresample();
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
}

fn bindgen_libswresample() {
    build_libswresample();

    println!("cargo:rustc-link-search=native=../FFmpeg/build/lib");
    println!("cargo:rustc-link-lib=static=swresample");

    let bindings = bindgen::Builder::default()
        .header("../FFmpeg/libswresample/swresample.h")
        .clang_arg("-I../FFmpeg/build/include")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .blocklist_item("FP_NAN")
        .blocklist_item("FP_INFINITE")
        .blocklist_item("FP_ZERO")
        .blocklist_item("FP_SUBNORMAL")
        .blocklist_item("FP_NORMAL")
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    bindings
        .write_to_file("./src/libswresample_bindings.rs")
        .expect("Couldn't write bindings!");
}

fn build_libswresample() {
    if std::path::Path::new("../FFmpeg/build/lib/libswresample.a").exists() {
        return;
    }
    // run ./configure
    let output = Command::new("./configure")
        .args([
            "--prefix=build",
            "--disable-programs",
            "--disable-avdevice",
            "--disable-avcodec",
            "--disable-avformat",
            "--disable-swscale",
            "--disable-postproc",
            "--disable-avfilter",
            "--disable-network",
            "--disable-pixelutils",
            "--disable-everything",
        ])
        .current_dir("../FFmpeg")
        .output()
        .expect("failed to ffmpeg configure");

    if !output.status.success() {
        panic!("failed to execute ffmpeg configure: {:?}", output);
    }

    // run make
    let output = Command::new("make")
        .current_dir("../FFmpeg")
        .output()
        .expect("failed to make ffmpeg");

    if !output.status.success() {
        panic!("failed to execute ffmpeg make: {:?}", output);
    }

    // run make install

    let output = Command::new("make")
        .args(["install"])
        .current_dir("../FFmpeg")
        .output()
        .expect("failed to make ffmpeg install");

    if !output.status.success() {
        panic!("failed to execute ffmpeg make install: {:?}", output);
    }
}
