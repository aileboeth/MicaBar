fn main() {
    // Run elevated: per-process speed needs a kernel ETW session, which needs admin.
    let attrs = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest")),
    );
    tauri_build::try_build(attrs).expect("failed to run tauri-build");
    // the exe icon resource (also used for the tray and windows) is compiled here: a changed
    // icon must rerun this script, or the old icon stays in the binary
    for f in ["icons/icon.ico", "icons/icon.png", "icons/32x32.png", "icons/128x128.png"] {
        println!("cargo:rerun-if-changed={f}");
    }
}
