use assert_cmd::Command;

#[test]
fn version_flag_prints_expected_version() {
    let mut cmd = Command::cargo_bin("ariadusage").unwrap();
    cmd.arg("--version");
    cmd.assert().success().stdout("ariadusage 0.0.0\n");
}
