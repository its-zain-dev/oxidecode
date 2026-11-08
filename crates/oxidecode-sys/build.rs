fn main() {
    let mut build = cc::Build::new();
    build
        .file("../../C/ocpty.c")
        .include("../../C")
        .flag_if_supported("-std=c11")
        .flag_if_supported("-O3")
        .flag_if_supported("-fno-plt")
        .warnings(true)
        .compile("oxidecode_native");
    println!("cargo:rerun-if-changed=../../C/ocpty.c");
    println!("cargo:rerun-if-changed=../../C/ocpty.h");
}
