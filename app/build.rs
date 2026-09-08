use std::{fs, path::{Path, PathBuf}};

fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_language(0x0409); // English (US)
        res.set_icon("../assets/Icon.ico");
        res.set("FileDescription", "Happy Wars Revived Self-hosted");
        res.set("ProductName", "Happy Wars Revived");
        res.compile().unwrap();
    }

    let manifest_dir_value = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_dir = Path::new(&manifest_dir_value);
    let profile = std::env::var("PROFILE").unwrap();
    let workspace_root = manifest_dir.parent().unwrap_or(manifest_dir);
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"));
    let output_dir = target_dir.join(&profile);

    copy_dir(Path::new("content"), &output_dir.join("content"));

    fs::copy("../assets/UWPInstaller.bat", output_dir.join("UWPInstaller.bat")).unwrap();

    println!("cargo:rerun-if-changed=content");
    println!("cargo:rerun-if-changed=../assets/UWPInstaller.bat");
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap().flatten() {
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}