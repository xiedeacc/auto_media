use std::process::Command;

fn main() {
    tauri_build::build();

    let hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_HASH={hash}");

    // Build timestamp in Beijing time (UTC+8), formatted to match the status bar.
    let beijing = chrono::Utc::now() + chrono::Duration::hours(8);
    println!(
        "cargo:rustc-env=BUILD_TIME={}",
        beijing.format("%Y%m%d %H:%M")
    );

    // Re-run when HEAD moves so the hash and build time stay current.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
}













