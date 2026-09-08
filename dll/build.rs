fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_language(0x0409);
        res.set("FileDescription", "Happy Wars UWP DLL Injector");
        res.set("ProductName", "Happy Wars Revived");
        res.compile().unwrap();
    }

    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let def = std::path::Path::new(&manifest).join("d3d11.def");

    assert!(def.exists(), "d3d11.def not found in {}", def.display());

    println!("cargo:rustc-link-arg=/DEF:{}", def.display());
    println!("cargo:rerun-if-changed=d3d11.def");
    println!("cargo:rerun-if-changed=build.rs");
}