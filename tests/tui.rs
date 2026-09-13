use std::process::Command;
#[test]
fn tui_rejects_redirected_io_without_terminal_escape_codes() {
    let result = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["tui", "--output", "/tmp/xteink-tui-unused"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("interactive terminal"));
    assert!(!result.stdout.contains(&27));
}
#[test]
fn tui_transfer_options_reject_conflicting_destinations() {
    let result = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args([
            "tui",
            "--output",
            "books",
            "--send-to",
            "reader.local",
            "--copy-to",
            "card",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("cannot be used with"));
}
