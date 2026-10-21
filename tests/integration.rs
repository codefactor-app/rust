use codefactor::{Codefactor, Error, ScanOptions};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "codefactor rust ; {}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        for (name, source) in [
            (
                "calls.php",
                "<?php function leaf(){return 1;} function caller(){return leaf();}",
            ),
            (
                "calls.py",
                "def leaf():\n    return 1\ndef caller(a, b, c):\n    return leaf()\n",
            ),
            (
                "calls.ts",
                "export function leaf(){return 1;} export function caller(){return leaf();}",
            ),
            (
                "calls.js",
                "function leaf(){return 1;} function caller(){return leaf();}",
            ),
            ("calls.rs", "fn leaf()->i32{1} fn caller()->i32{leaf()}"),
        ] {
            fs::write(root.join(name), source).unwrap();
        }
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn embedded_engine_scans_all_languages_and_queries() {
    let fixture = Fixture::new();
    let client = Codefactor::new().unwrap();
    let report = client.scan(&fixture.0, &ScanOptions::default()).unwrap();
    assert_eq!(report.data["summary"]["files"], 5);
    assert_eq!(report.data["summary"]["functions"], 10);
    assert_eq!(report.data["parse_errors"], serde_json::json!([]));
    let filtered = client
        .scan(
            &fixture.0,
            &ScanOptions {
                languages: vec!["python".into()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(filtered.data["summary"]["files"], 1);
    assert!(client.ast(fixture.0.join("calls.py")).unwrap().data["root"].is_object());
    assert!(!client
        .graph("calls.caller", &fixture.0, "callees", Some(2))
        .unwrap()
        .data["edges"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn violations_are_reports_and_configuration_failures_are_errors() {
    let fixture = Fixture::new();
    let client = Codefactor::new().unwrap();
    let config = fixture.0.join("limits.yaml");
    fs::write(
        &config,
        "limits:\n  function_params: {warn: 1, error: 2}\ncache:\n  enabled: false\n",
    )
    .unwrap();
    let options = ScanOptions {
        config: Some(config.clone()),
        ..Default::default()
    };
    let report = client.scan(&fixture.0, &options).unwrap();
    assert_eq!(report.exit_code, 2);
    assert!(report.data["summary"]["errors"].as_u64().unwrap() > 0);
    fs::write(config, "[invalid: yaml").unwrap();
    assert!(matches!(
        client.scan(&fixture.0, &options),
        Err(Error::Process {
            exit_code: Some(3),
            ..
        })
    ));
    assert!(client
        .scan(fixture.0.join("missing"), &Default::default())
        .is_err());
}

#[cfg(unix)]
#[test]
fn child_is_stopped_on_timeout() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let fake = fixture.0.join("slow");
    fs::write(
        &fake,
        "#!/usr/bin/env python3\nimport time\ntime.sleep(30)\n",
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let client = Codefactor::with_binary(fake).with_timeout(Duration::from_millis(50));
    assert!(matches!(
        client.scan(&fixture.0, &Default::default()),
        Err(Error::Timeout)
    ));
}
