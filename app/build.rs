//! On macOS, builds swift/DeviceKeys.swift (the person's device keys in the Secure Enclave, src/device_key.rs) into a
//! static library linked into the app, so the code that signs is the app's own, under its signature. CryptoKit, which
//! keeps an enclave key as a sealed block with no keychain item, is Swift only. Elsewhere it does nothing.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=swift/DeviceKeys.swift");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo gives OUT_DIR"));
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("no Swift target for the architecture {other:?}"),
    };
    let lowest = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".into());
    let lib = out.join("libjaynes_device_keys.a");
    let built = Command::new("xcrun")
        .args(["swiftc", "-parse-as-library", "-static", "-emit-library", "-O", "-module-name", "JaynesDeviceKeys"])
        .args(["-target", &format!("{arch}-apple-macos{lowest}"), "swift/DeviceKeys.swift", "-o"])
        .arg(&lib)
        .status()
        .expect("cannot run xcrun swiftc: the app's device keys need Xcode's Swift");
    assert!(built.success(), "swiftc could not build swift/DeviceKeys.swift");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=jaynes_device_keys");
    // the Swift runtime is the system's (macOS 10.14.4 on): the SDK's stubs to link it, the toolchain's libraries for
    // what older systems lack
    let sdk = run(&["--show-sdk-path"]);
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    let swiftc = PathBuf::from(run(&["--find", "swiftc"]));
    if let Some(toolchain) = swiftc.parent().and_then(|bin| bin.parent()) {
        println!("cargo:rustc-link-search=native={}", toolchain.join("lib/swift/macosx").display());
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    for framework in ["CryptoKit", "LocalAuthentication", "Security", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

fn run(args: &[&str]) -> String {
    let out = Command::new("xcrun").args(args).output().expect("cannot run xcrun");
    assert!(out.status.success(), "xcrun {args:?} failed");
    String::from_utf8(out.stdout).expect("xcrun says UTF-8").trim().to_owned()
}
