// Embed the exact manifest bytes in both toolchains. Do not let the linker merge XML:
// LLVM's manifest merger can change the namespace of trustInfo and its attributes.
fn main() {
    println!("cargo:rerun-if-changed=snip.manifest");
    println!("cargo:rerun-if-env-changed=WINDRES");
    println!("cargo:rerun-if-env-changed=RC");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") { return; }
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let manifest = root.join("snip.manifest");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let rc = out.join("snip.rc");
    let escaped = manifest.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let icon = root.join("resources/snip.ico").to_string_lossy().replace('\\', "/");
    let icon_off = root.join("resources/snip-off.ico").to_string_lossy().replace('\\', "/");
    println!("cargo:rerun-if-changed=resources/snip.ico");
    println!("cargo:rerun-if-changed=resources/snip-off.ico");
    std::fs::write(&rc, format!("1 24 \"{escaped}\"\n1 ICON \"{icon}\"\n2 ICON \"{icon_off}\"\n")).expect("write manifest resource script");
    let host = std::env::var("HOST").unwrap_or_default();
    if target.ends_with("windows-msvc") {
        let resource = out.join("snip-manifest.res");
        let compiler = std::env::var("RC").unwrap_or_else(|_| {
            if host.contains("windows") { "rc".into() } else { "llvm-rc".into() }
        });
        let result = std::process::Command::new(&compiler)
            .arg("/nologo").arg("/fo").arg(&resource).arg(&rc)
            .status().unwrap_or_else(|e| panic!("run {compiler} to embed snip.manifest: {e}"));
        assert!(result.success(), "manifest resource compiler failed");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
        // Load the DLLs snip imports from System32 only, never from the folder the exe sits in.
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
        println!("cargo:rustc-link-arg-bins={}", resource.display());
        if matches!(std::env::var("DEBUG").as_deref(), Ok("false" | "0" | "none")) {
            println!("cargo:rustc-link-arg-bins=/DEBUG:NONE");
        }
    } else if target.ends_with("windows-gnu") {
        let resource = out.join("snip-manifest.o");
        let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
            if host.contains("windows") { "windres".into() }
            else { format!("{}-w64-mingw32-windres", target.split('-').next().unwrap()) }
        });
        let result = std::process::Command::new(&windres)
            .arg("--input").arg(&rc).arg("--output").arg(&resource).arg("--output-format=coff")
            .status().unwrap_or_else(|e| panic!("run {windres} to embed snip.manifest: {e}"));
        assert!(result.success(), "manifest resource compiler failed");
        println!("cargo:rustc-link-arg-bins={}", resource.display());
    }
}
