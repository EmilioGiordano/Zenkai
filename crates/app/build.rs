fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    embed_windows_resources()?;
    Ok(())
}

#[cfg(windows)]
fn embed_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    use std::env;
    use std::path::PathBuf;

    let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("zenkai.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = env::var("CARGO_PKG_VERSION")?;
    let numeric = format!(
        "{},{},{},0",
        env::var("CARGO_PKG_VERSION_MAJOR")?,
        env::var("CARGO_PKG_VERSION_MINOR")?,
        env::var("CARGO_PKG_VERSION_PATCH")?,
    );
    let icon_path = icon.display().to_string().replace('\\', "\\\\");
    // GPUI loads icon resource 1 for the window and taskbar; GPUI's own manifest is
    // embedded by its build script, so this file must not declare one.
    let script = format!(
        r#"1 ICON "{icon_path}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Zenkai"
      VALUE "FileDescription", "Zenkai"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "zenkai"
      VALUE "LegalCopyright", "Licensed under Apache-2.0"
      VALUE "OriginalFilename", "zenkai.exe"
      VALUE "ProductName", "Zenkai"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let resource = PathBuf::from(env::var("OUT_DIR")?).join("zenkai.rc");
    std::fs::write(&resource, script)?;
    embed_resource::compile(&resource, embed_resource::NONE).manifest_required()?;
    Ok(())
}
