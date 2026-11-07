// Generates one `allow-<command>` permission per command of the plugin, and the Swift
// package wiring from `ios/`. No default permission is declared: see the note in `lib.rs`.
//
// On macOS, compiles the Swift native bridge from `macos/Sources/` into a static library
// and links the required Apple frameworks.
const COMMANDS: &[&str] = &["status", "gyroscope_sample", "bluetooth_scan"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        compile_macos_swift();
    }
}

fn compile_macos_swift() {
    use std::env;
    use std::path::PathBuf;
    use std::process::Command;

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let swift_dir = manifest_dir.join("macos").join("Sources");

    // Collect all Swift source files.
    let swift_files: Vec<PathBuf> = std::fs::read_dir(&swift_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", swift_dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "swift"))
        .collect();

    if swift_files.is_empty() {
        return;
    }

    // Rebuild when any Swift file changes.
    for file in &swift_files {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    // macOS SDK path.
    let sdk_output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("xcrun failed — is Xcode or Command Line Tools installed?");
    let sdk = String::from_utf8_lossy(&sdk_output.stdout)
        .trim()
        .to_string();

    // Target triple: match the Cargo target architecture.
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "aarch64".to_string());
    let target = match arch.as_str() {
        "aarch64" => "arm64-apple-macosx13.0",
        "x86_64" => "x86_64-apple-macosx13.0",
        other => panic!("unsupported macOS architecture: {other}"),
    };

    let lib_path = out_dir.join("libthis_device_macos.a");

    let mut cmd = Command::new("swiftc");
    cmd.arg("-emit-library")
        .arg("-static")
        .arg("-module-name")
        .arg("ThisDeviceMacOS")
        .arg("-target")
        .arg(target)
        .arg("-sdk")
        .arg(&sdk)
        .arg("-O")
        .arg("-whole-module-optimization")
        .arg("-o")
        .arg(&lib_path);

    for file in &swift_files {
        cmd.arg(file);
    }

    let output = cmd.output().expect("swiftc failed to execute");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "swiftc compilation failed (status {}):\n--- stderr ---\n{stderr}\n--- stdout ---\n{stdout}",
            output.status
        );
    }

    // Tell cargo to link the static library.
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=this_device_macos");

    // Link the Apple frameworks used by the Swift code.
    for framework in [
        "AVFoundation",
        "CoreLocation",
        "Contacts",
        "CoreBluetooth",
        "LocalAuthentication",
        "UserNotifications",
        "EventKit",
        "Speech",
        "Photos",
        "PhotosUI",
        "AppKit",
        "Network",
        "Foundation",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
