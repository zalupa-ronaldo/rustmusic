use std::{path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTMUSIC_COMMIT");
    println!("cargo:rerun-if-env-changed=RUSTMUSIC_DIRTY");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let git_dir = manifest.join("../../.git");
    let head = git_dir.join("HEAD");
    if head.is_file() {
        println!("cargo:rerun-if-changed={}", head.display());
        if let Ok(text) = std::fs::read_to_string(&head)
            && let Some(reference) = text.trim().strip_prefix("ref: ")
        {
            let ref_file = git_dir.join(reference);
            if ref_file.is_file() {
                println!("cargo:rerun-if-changed={}", ref_file.display());
            }
        }
    }
    let dirty = match std::env::var("RUSTMUSIC_DIRTY") {
        Ok(value) => matches!(value.trim(), "1" | "true" | "TRUE"),
        Err(_) => git_dirty(),
    };
    let commit = std::env::var("RUSTMUSIC_COMMIT")
        .ok()
        .and_then(|value| sanitize(value.trim()))
        .or_else(git_short)
        .unwrap_or_else(|| "unknown".to_owned());
    let label = if dirty && !commit.ends_with("-dirty") {
        format!("{commit}-dirty")
    } else {
        commit
    };
    println!("cargo:rustc-env=RUSTMUSIC_COMMIT={label}");
}

fn sanitize(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let (core, marked_dirty) = match raw.strip_suffix("-dirty") {
        Some(core) => (core, true),
        None => (raw, false),
    };
    let hex = (7..=40).contains(&core.len()) && core.bytes().all(|byte| byte.is_ascii_hexdigit());
    if core != "unknown" && !hex {
        return None;
    }
    if marked_dirty {
        Some(format!("{core}-dirty"))
    } else {
        Some(core.to_owned())
    }
}

fn git_short() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    sanitize(text.trim())
}

fn git_dirty() -> bool {
    let Ok(output) = Command::new("git").args(["status", "--porcelain"]).output() else {
        return false;
    };
    output.status.success() && output.stdout.iter().any(|byte| !byte.is_ascii_whitespace())
}
