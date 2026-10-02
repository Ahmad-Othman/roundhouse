#[cfg(unix)]
#[test]
fn local_verify_cli_preserves_plans_failures_and_scope() {
    let result = std::process::Command::new("ruby")
        .arg("tests/rh_verify_test.rb")
        .output()
        .expect("bin/rh verification tests require Ruby");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
