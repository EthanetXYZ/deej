use std::process::Command;

/// Runs git and returns its trimmed output, or `None` (no git, or not a checkout).
fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn main() {
    // the short commit hash shows up in the logs and the about box, like the Go builds' -X main.gitCommit
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=DEEJ_GIT_COMMIT={commit}");

    // rebuild when the commit changes: HEAD names the branch, and the branch's ref file changes with every commit
    // (git finds both, wherever the checkout's .git is)
    if let Some(head) = git(&["rev-parse", "--path-format=absolute", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &branch]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    tauri_build::build();
}
