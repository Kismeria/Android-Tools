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

/// Builds the DirectShow camera (softcam) for Windows 10, for both 64- and 32-bit host apps.
fn build_softcam(target: &str, out_name: &str) {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("softcam");
    println!("cargo:rerun-if-changed=softcam");
    let obj = PathBuf::from(env::var("OUT_DIR").unwrap()).join(format!("softcam-{target}"));
    std::fs::create_dir_all(&obj).unwrap();
    let mut sources = Vec::new();
    for dir in ["baseclasses", "softcamcore", "softcam"] {
        for e in std::fs::read_dir(root.join(dir)).unwrap().flatten() {
            if e.path().extension().map(|x| x == "cpp").unwrap_or(false) {
                sources.push(e.path());
            }
        }
    }
    let cl = cc::windows_registry::find_tool(target, "cl.exe").expect("MSVC cl.exe not found");
    let status = cl
        .to_command()
        .current_dir(&obj)
        .env("VSLANG", "1033")
        .args(["/nologo", "/LD", "/EHsc", "/O2", "/MT", "/std:c++17", "/utf-8", "/W0"])
        .args(["/DWIN32", "/DNDEBUG", "/D_CRT_SECURE_NO_WARNINGS", "/DUNICODE", "/D_UNICODE"])
        .arg(format!("/I{}", root.display()))
        .arg(format!("/I{}", root.join("baseclasses").display()))
        .args(&sources)
        .arg(format!("/Fe{}", obj.parent().unwrap().join(out_name).display()))
        .arg(format!("/Fo{}\\", obj.display()))
        .args(["/link", "strmiids.lib", "winmm.lib", "ole32.lib", "oleaut32.lib", "advapi32.lib", "user32.lib", "gdi32.lib", "uuid.lib"])
        .arg(format!("/DEF:{}", root.join("softcam").join("softcam.def").display()))
        .status()
        .expect("failed to run cl.exe");
    assert!(status.success(), "softcam build failed for {target}");
}

fn main() {
    build_camera_dll();
    build_softcam("x86_64-pc-windows-msvc", "softcam64.dll");
    build_softcam("i686-pc-windows-msvc", "softcam32.dll");
    println!("cargo:rerun-if-changed=embed");
    tauri_build::build();
}
