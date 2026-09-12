//! Embeds the application icon and compilation-date-based version info into the Windows executable.

use chrono::Datelike;

fn main() {
    let now = chrono::Local::now();
    let day = now.ordinal(); // 1..=366
    let year_yy = now.year().rem_euclid(100);
    let build_version = format!("{year_yy:02}.{day}");

    println!("cargo:rustc-env=APP_BUILD_VERSION={build_version}");
    // No rerun-if-changed on purpose: with it, cargo caches the stamp and the
    // day-of-year version number freezes at the first build of the day. Without
    // any directive the script re-runs on every build, so the version always
    // carries the current {yy}.{ordinal}.

    #[cfg(windows)]
    {
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
        let rc_path = std::path::Path::new(&out_dir).join("tokpaek.rc");
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        let icon_path = std::path::Path::new(&manifest_dir).join("assets/tokpaek.ico");
        let icon_str = icon_path.display().to_string().replace('\\', "/");

        let rc_content = format!(
            r#"1 ICON "{icon_str}"

1 VERSIONINFO
FILEVERSION {year_yy},{day},0,0
PRODUCTVERSION {year_yy},{day},0,0
FILEOS 0x4
FILETYPE 0x1
{{
  BLOCK "StringFileInfo"
  {{
    BLOCK "040904B0"
    {{
      VALUE "CompanyName", "Brent"
      VALUE "FileDescription", "Tokpaek - AI quota strip"
      VALUE "FileVersion", "{build_version}"
      VALUE "InternalName", "tokpaek"
      VALUE "OriginalFilename", "tokpaek.exe"
      VALUE "ProductName", "Tokpaek"
      VALUE "ProductVersion", "{build_version}"
      VALUE "LegalCopyright", "Brent"
    }}
  }}
  BLOCK "VarFileInfo"
  {{
    VALUE "Translation", 0x409, 1200
  }}
}}
"#
        );
        std::fs::write(&rc_path, rc_content).expect("failed to write tokpaek.rc");
        embed_resource::compile(&rc_path, embed_resource::NONE);
    }
}
