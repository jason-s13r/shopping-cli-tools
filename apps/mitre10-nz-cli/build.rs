fn main() {
    build_kit::emit::Stamper::new("M10")
        .tag_glob("mitre10-nz-cli/v*")
        .emit()
        .expect("stamping the build");
}
