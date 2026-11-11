fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/snapshot/webkit.m")
            .file("src/glass.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("faderwake_snapshots");
        println!("cargo:rerun-if-changed=src/snapshot/webkit.m");
        println!("cargo:rerun-if-changed=src/glass.m");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=CoreImage");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
        println!("cargo:rustc-link-lib=framework=WebKit");
    }
    tauri_build::build();
}
