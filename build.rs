fn main() {
    println!("cargo:rerun-if-changed=assets/vibe.ico");
    println!("cargo:rerun-if-changed=release/update-public-key.txt");
    let version = std::env::var("CARGO_PKG_VERSION").expect("package version");
    println!("cargo:rerun-if-env-changed=RC_PATH");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // winresource discovers the newest Windows SDK through its registry entries.
    // RC_PATH can explicitly select rc.exe for custom SDK installations.
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/vibe.ico")
        .set("ProductName", "VIBE")
        .set("FileDescription", "VIBE AI Workspace")
        .set("InternalName", "VIBE")
        .set("OriginalFilename", "VIBE.exe")
        .set("FileVersion", &version)
        .set("ProductVersion", &version);
    resource.compile_for("vibe").unwrap_or_else(|error| {
        panic!(
            "Cannot build VIBE Windows icon/version resources: {error}. Install the Windows SDK resource compiler (rc.exe), or set RC_PATH to its full path."
        )
    });
}
