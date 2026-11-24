fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    embed_windows_resources()?;
    Ok(())
}

#[cfg(windows)]
fn embed_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    use std::env;
    use std::path::PathBuf;

    let crate_folder = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    // The app's icon file, so replacing that one file changes both programs.
    let icon = crate_folder
        .parent()
        .ok_or("the crate has no parent folder")?
        .join("app")
        .join("zenkai.ico");
    let manifest = crate_folder.join("zenkai-launcher.manifest");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", manifest.display());

    let version = env::var("CARGO_PKG_VERSION")?;
    let numeric = format!(
        "{},{},{},0",
        env::var("CARGO_PKG_VERSION_MAJOR")?,
        env::var("CARGO_PKG_VERSION_MINOR")?,
        env::var("CARGO_PKG_VERSION_PATCH")?,
    );
    let escaped = |path: &PathBuf| path.display().to_string().replace('\\', "\\\\");
    let (icon_path, manifest_path) = (escaped(&icon), escaped(&manifest));
    let script = format!(
        r#"1 ICON "{icon_path}"
1 24 "{manifest_path}"
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
      VALUE "InternalName", "zenkai-launcher"
      VALUE "LegalCopyright", "Licensed under AGPL-3.0-only"
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
    let resource = PathBuf::from(env::var("OUT_DIR")?).join("zenkai-launcher.rc");
    std::fs::write(&resource, script)?;
    embed_resource::compile(&resource, embed_resource::NONE).manifest_required()?;
    Ok(())
}
