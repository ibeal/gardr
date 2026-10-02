use std::fs;
use std::path::Path;

fn check_rust_files(directory: &Path) {
    for entry in fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            check_rust_files(&path);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let bytes = fs::read(&path).expect("read Rust source");
            let lines = bytes.iter().filter(|byte| **byte == b'\n').count()
                + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
            assert!(
                lines < 600,
                "{} has {lines} physical lines (limit: 599)",
                path.display()
            );
        }
    }
}

#[test]
fn every_rust_source_file_stays_under_600_physical_lines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    check_rust_files(&root.join("src"));
    check_rust_files(&root.join("tests"));
}
