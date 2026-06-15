use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=HOLLA_VERSION_OVERRIDE");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    let version = std::env::var("HOLLA_VERSION_OVERRIDE").unwrap_or_else(|_| {
        let cargo_version =
            std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
        let short_sha = Command::new("git")
            .args(["rev-parse", "--short=7", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|sha| sha.trim().to_owned());

        short_sha.map_or_else(
            || cargo_version.clone(),
            |sha| format!("{cargo_version}+{sha}"),
        )
    });

    println!("cargo:rustc-env=HOLLA_VERSION={version}");
}
