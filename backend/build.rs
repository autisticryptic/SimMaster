//! Build script for injecting version and Git information at compile time

fn main() {
    // Never inject a made-up version when metadata is missing or drifted.
    let version = std::fs::read_to_string("../VERSION")
        .expect("VERSION is required for a reproducible build")
        .trim()
        .to_string();
    let package_version = std::env::var("CARGO_PKG_VERSION")
        .expect("Cargo must provide the package version to build.rs");
    assert_eq!(
        version, package_version,
        "VERSION and backend/Cargo.toml must be synchronized before building"
    );

    // Get Git branch name
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Get Git commit hash (short)
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Set compile-time environment variables
    println!("cargo:rustc-env=APP_VERSION={}", version);
    println!("cargo:rustc-env=GIT_BRANCH={}", branch);
    println!("cargo:rustc-env=GIT_COMMIT={}", commit);

    // Rebuild if VERSION file changes
    println!("cargo:rerun-if-changed=../VERSION");
    // Rebuild if git HEAD changes (new commits)
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads/");
}
