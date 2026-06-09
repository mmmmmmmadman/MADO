// Build script to compile RtAudio wrapper and link RtAudio

fn main() {
    println!("cargo:rerun-if-changed=rtaudio_wrapper.cpp");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Embed Windows application icon into the exe (taskbar / explorer).
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/icons/mado.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/mado.ico");
        res.compile().unwrap();
    }

    // Compile C++ wrapper
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("rtaudio_wrapper.cpp")
        .flag_if_supported("-std=c++17")
        .flag_if_supported("/std:c++17") // MSVC
        .flag_if_supported("-Wno-unused-parameter");

    // Platform-specific include paths
    if target_os == "macos" {
        build.include("/opt/homebrew/include"); // Homebrew ARM64
        build.include("/usr/local/include"); // Homebrew Intel
    } else if target_os == "windows" {
        // Support RTAUDIO_DIR env var
        if let Ok(dir) = std::env::var("RTAUDIO_DIR") {
            build.include(format!("{}/include", dir));
        }
        // Support vcpkg
        if let Ok(vcpkg_root) = std::env::var("VCPKG_ROOT") {
            let triplet = std::env::var("VCPKG_DEFAULT_TRIPLET")
                .unwrap_or_else(|_| "x64-windows".to_string());
            let installed = format!("{}/installed/{}", vcpkg_root, triplet);
            build.include(format!("{}/include", installed));
        }
    }

    build.compile("rtaudio_wrapper");

    // Link RtAudio library
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=dylib=rtaudio");
        println!("cargo:rustc-link-search=/opt/homebrew/lib");
        println!("cargo:rustc-link-search=/usr/local/lib");
    } else if target_os == "linux" {
        println!("cargo:rustc-link-lib=dylib=rtaudio");
        println!("cargo:rustc-link-lib=dylib=pulse");
        println!("cargo:rustc-link-lib=dylib=jack");
    } else if target_os == "windows" {
        if let Ok(dir) = std::env::var("RTAUDIO_DIR") {
            println!("cargo:rustc-link-search={}/lib", dir);
        }
        if let Ok(vcpkg_root) = std::env::var("VCPKG_ROOT") {
            let triplet = std::env::var("VCPKG_DEFAULT_TRIPLET")
                .unwrap_or_else(|_| "x64-windows".to_string());
            println!(
                "cargo:rustc-link-search={}/installed/{}/lib",
                vcpkg_root, triplet
            );
        }
        println!("cargo:rustc-link-lib=dylib=rtaudio");
        println!("cargo:rustc-link-lib=dylib=dsound");
        println!("cargo:rustc-link-lib=dylib=ole32");
        println!("cargo:rustc-link-lib=dylib=winmm");
    }
}
