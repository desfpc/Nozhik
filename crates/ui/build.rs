//! Capture non-sensitive build identity for the About dialog.
use std::process::Command;
fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    for (name, args) in [
        ("NOZHIK_COMMIT", vec!["rev-parse", "--short=12", "HEAD"]),
        ("NOZHIK_RUSTC", vec!["--version"]),
    ] {
        let program = if name == "NOZHIK_COMMIT" {
            "git"
        } else {
            "rustc"
        };
        let value = Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map_or_else(
                || "unknown".into(),
                |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
            );
        println!("cargo:rustc-env={name}={value}");
    }
}
