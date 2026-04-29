use std::{env, fs, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=FROID_VERSION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    watch_git_head_ref();

    let version = env::var("FROID_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_short_sha)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=FROID_VERSION={version}");
}

fn watch_git_head_ref() {
    let Ok(head) = fs::read_to_string(".git/HEAD") else {
        return;
    };

    let Some(ref_path) = head.strip_prefix("ref: ").map(str::trim) else {
        return;
    };

    if !ref_path.is_empty() {
        println!("cargo:rerun-if-changed=.git/{ref_path}");
    }
}

fn git_short_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let sha = String::from_utf8(output.stdout).ok()?;
    let sha = sha.trim();

    (!sha.is_empty()).then(|| sha.to_string())
}
