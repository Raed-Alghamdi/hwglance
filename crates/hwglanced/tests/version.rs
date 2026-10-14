//! `hwglanced --version` prints the package version (bug reports ask for it).

#[test]
fn version_flag_prints_name_and_version() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_hwglanced"))
        .arg("--version")
        .output()
        .expect("run hwglanced");
    assert!(out.status.success());
    let expected = format!("hwglanced {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}
