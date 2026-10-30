fn main() {
    if std::path::Path::new("vendor/sonic/sonic.c").exists() {
        cc::Build::new()
            .file("vendor/sonic/sonic.c")
            .include("vendor/sonic")
            .compile("reader_sonic");
        println!("cargo:rerun-if-changed=vendor/sonic/sonic.c");
        println!("cargo:rerun-if-changed=vendor/sonic/sonic.h");
    }
}
