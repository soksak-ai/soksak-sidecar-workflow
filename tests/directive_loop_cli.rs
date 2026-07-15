use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_directive-loop"))
        .args(args)
        .output()
        .expect("directive-loop process")
}

#[test]
fn store_path_is_required_and_absolute_before_provider_startup() {
    for args in [
        vec!["a directive"],
        vec!["a directive", "--store", "relative/ledger.json"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--store requires an absolute path")
        );
    }
}
