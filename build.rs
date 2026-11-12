fn resource_directory(path: &str) -> String {
    path.replace('\\', "/")
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let dir = resource_directory(&std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=win-island.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}/win-island.manifest");
    println!("cargo:rerun-if-changed=assets/brand/tidemark.ico");
    println!("cargo:rerun-if-changed=assets/wallpapers/coast-preview.bmp");
    println!("cargo:rerun-if-changed=assets/wallpapers/night-preview.bmp");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc = out.join("tidemark.rc");
    let res = out.join("tidemark.res");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let file_version = format!(
        "{},{},{},0",
        std::env::var("CARGO_PKG_VERSION_MAJOR").unwrap(),
        std::env::var("CARGO_PKG_VERSION_MINOR").unwrap(),
        std::env::var("CARGO_PKG_VERSION_PATCH").unwrap()
    );
    std::fs::write(&rc, format!(
        "101 ICON \"{dir}/assets/brand/tidemark.ico\"\n201 BITMAP \"{dir}/assets/wallpapers/coast-preview.bmp\"\n202 BITMAP \"{dir}/assets/wallpapers/night-preview.bmp\"\n\
         1 VERSIONINFO\nFILEVERSION {file_version}\nPRODUCTVERSION {file_version}\nBEGIN\nBLOCK \"StringFileInfo\"\nBEGIN\nBLOCK \"040904B0\"\nBEGIN\n\
         VALUE \"FileDescription\", \"Tidemark\"\nVALUE \"ProductName\", \"Tidemark\"\nVALUE \"FileVersion\", \"{version}\"\nVALUE \"ProductVersion\", \"{version}\"\nEND\nEND\n\
         BLOCK \"VarFileInfo\"\nBEGIN\nVALUE \"Translation\", 0x0409, 1200\nEND\nEND\n"
    )).unwrap();
    let sdk =
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:/Program Files (x86)".into());
    let bins = std::path::PathBuf::from(sdk).join("Windows Kits/10/bin");
    let mut candidates: Vec<_> = std::fs::read_dir(bins)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("x64/rc.exe"))
        .filter(|p| p.exists())
        .collect();
    candidates.sort();
    let compiler = candidates
        .last()
        .expect("Windows SDK resource compiler (rc.exe) required");
    let status = std::process::Command::new(compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc)
        .status()
        .unwrap();
    assert!(status.success(), "Tidemark resources failed to compile");
    println!("cargo:rustc-link-arg-bin=win-island={}", res.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_runner_paths_do_not_contain_resource_escape_sequences() {
        assert_eq!(
            resource_directory(r"D:\a\tidemark\tidemark"),
            "D:/a/tidemark/tidemark"
        );
        assert_eq!(
            resource_directory(r"C:\work with spaces\app"),
            "C:/work with spaces/app"
        );
    }
}
