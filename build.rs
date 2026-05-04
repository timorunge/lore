fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    let base = std::env::var("CARGO_PKG_VERSION").unwrap();
    let version = git_version(&base);
    println!("cargo:rustc-env=LORE_VERSION={version}");
    println!("cargo:rustc-env=LORE_USER_AGENT=lore/{base}");
}

fn git_version(base: &str) -> String {
    let is_tagged = std::process::Command::new("git")
        .args(["describe", "--exact-match", "--tags", "HEAD"])
        .output()
        .is_ok_and(|o| o.status.success());

    if is_tagged {
        return format!("v{base}");
    }

    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned());

    match hash {
        Some(h) if !h.is_empty() => format!("v{base}-{h}"),
        _ => format!("v{base}"),
    }
}
