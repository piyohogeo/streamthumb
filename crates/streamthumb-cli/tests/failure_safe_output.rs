use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn create() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "streamthumb-cli-process-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("the test directory must be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn malformed_input_preserves_existing_output_and_removes_staging_file() {
    let directory = TestDirectory::create();
    let input = directory.path().join("malformed.png");
    let output = directory.path().join("thumbnail.png");
    fs::write(&input, b"not a PNG").expect("the malformed input must be written");
    fs::write(&output, b"existing output").expect("the existing output must be written");

    let result = Command::new(env!("CARGO_BIN_EXE_streamthumb"))
        .args([&input, &output])
        .output()
        .expect("the CLI process must start");

    assert!(!result.status.success());
    assert_eq!(
        fs::read(&output).expect("the existing output must remain readable"),
        b"existing output"
    );
    let mut names = fs::read_dir(directory.path())
        .expect("the test directory must remain readable")
        .map(|entry| {
            entry
                .expect("the directory entry must be readable")
                .file_name()
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["malformed.png", "thumbnail.png"]);
}
