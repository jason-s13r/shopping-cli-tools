fn main() {
    build_kit::emit::Stamper::new("FMNZ")
        .tag_glob("farmers-nz-cli/v*")
        .emit()
        .expect("stamping the build");
}
