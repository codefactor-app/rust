#![cfg(unix)]
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

#[test]
fn commands_preserve_arguments_streams_cwd_and_status() {
    let root = std::env::temp_dir().join(format!("codefactor cli ; {}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fake = root.join("fake engine");
    fs::write(
        &fake,
        "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\"\ncat\nprintf 'raw diagnostic\\n' >&2\nexit 7\n",
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let args = [
        "scan",
        "--future-option",
        "path ; $(literal)",
        "--",
        "quoted \"value\"",
    ];
    for (launcher, cargo) in [
        (env!("CARGO_BIN_EXE_codefactor"), false),
        (env!("CARGO_BIN_EXE_cargo-codefactor"), true),
    ] {
        let mut command = Command::new(launcher);
        if cargo {
            command.arg("codefactor");
        }
        let mut child = command
            .args(args)
            .current_dir(&root)
            .env("CODEFACTOR_BINARY", &fake)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"stdin data\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "{}\n{}\nstdin data\n",
                root.canonicalize().unwrap().display(),
                args.join("\n")
            )
        );
        assert_eq!(output.stderr, b"raw diagnostic\n");
        let missing = Command::new(launcher)
            .args(if cargo {
                vec!["codefactor", "--version"]
            } else {
                vec!["--version"]
            })
            .env("CODEFACTOR_BINARY", root.join("missing"))
            .output()
            .unwrap();
        assert_eq!(missing.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&missing.stderr).contains("Cannot start Codefactor binary"));
    }
    fs::remove_dir_all(root).unwrap();
}
