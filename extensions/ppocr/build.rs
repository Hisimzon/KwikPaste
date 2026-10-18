use std::{path::PathBuf, process::Command};

/// Fetch only checksum-verified official models, and rerun when the pins change.
fn main() {
    println!("cargo:rerun-if-changed=models.json");
    println!("cargo:rerun-if-changed=fetch_models.py");
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    for tier in ["tiny", "small", "medium"] {
        for role in ["det", "rec"] {
            for suffix in ["onnx", "yml"] {
                println!("cargo:rerun-if-changed=.cache/models/{tier}/{role}/inference.{suffix}");
            }
        }
    }
    let status = Command::new("python")
        .arg(root.join("fetch_models.py"))
        .current_dir(root)
        .status()
        .expect("Python is required to fetch pinned PP-OCR models");
    assert!(
        status.success(),
        "PP-OCR model fetch/checksum validation failed"
    );
}
