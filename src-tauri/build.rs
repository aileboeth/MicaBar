fn main() {
    // Run elevated: per-process speed needs a kernel ETW session, which needs admin.
    let attrs = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest")),
    );
    tauri_build::try_build(attrs).expect("failed to run tauri-build");
}
