use std::{env, path::PathBuf};

/// Builds the Media Foundation camera source DLL with MSVC so it can be embedded in the exe.
fn build_camera_dll() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let vcam = manifest.join("vcam");
    for f in ["camsource.cpp", "shared.h", "camsource.def"] {
        println!("cargo:rerun-if-changed=vcam/{f}");
    }
    let cl = cc::windows_registry::find_tool(&target, "cl.exe").expect("MSVC cl.exe not found");
    let status = cl
        .to_command()
        .current_dir(&out)
        .env("VSLANG", "1033")
        .args(["/nologo", "/LD", "/EHsc", "/O2", "/MT", "/std:c++17", "/DUNICODE", "/D_UNICODE", "/W3", "/utf-8"])
        .arg(vcam.join("camsource.cpp"))
        .arg(format!("/Fe{}", out.join("AndroidToolsCam.dll").display()))
        .arg(format!("/Fo{}\\", out.display()))
        .args(["/link", "mfplat.lib", "mf.lib", "mfuuid.lib", "mfsensorgroup.lib", "ole32.lib", "advapi32.lib"])
        .arg(format!("/DEF:{}", vcam.join("camsource.def").display()))
        .status()
        .expect("failed to run cl.exe");
    assert!(status.success(), "camera DLL build failed");
}

fn main() {
    build_camera_dll();
    println!("cargo:rerun-if-changed=embed");
    tauri_build::build();
}
