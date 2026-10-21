//! Internal package management, independent of the embedded core version.
use crate::{release, Error};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub struct Context {
    pub config: Option<PathBuf>,
    pub root: Option<PathBuf>,
}

pub fn context(args: &[OsString]) -> Context {
    let mut result = Context::default();
    let mut positional = Vec::new();
    let mut i = 0;
    const VALUES: &[&str] = &[
        "--config",
        "--file",
        "--root",
        "--format",
        "--out",
        "--lang",
        "--limit",
        "--group-by",
        "--function",
        "--from",
        "--to",
        "--depth",
        "--max-paths",
        "--architecture-profile",
        "--log-level",
        "--color",
        "--port",
        "--host",
        "--a",
        "--b",
        "--category",
    ];
    while i < args.len() {
        let arg = args[i].to_string_lossy();
        if arg == "--" {
            positional.extend_from_slice(&args[i + 1..]);
            break;
        }
        if arg.starts_with("--") {
            let (key, inline) = arg
                .split_once('=')
                .map_or((arg.as_ref(), None), |(k, v)| (k, Some(v)));
            if VALUES.contains(&key) {
                let value = if let Some(v) = inline {
                    Some(OsString::from(v))
                } else {
                    i += 1;
                    args.get(i).cloned()
                };
                if key == "--config"
                    || (key == "--file"
                        && positional.first().is_some_and(|v: &OsString| v == "config"))
                {
                    result.config = value.clone().map(PathBuf::from);
                }
                if key == "--root" {
                    result.root = value.map(PathBuf::from);
                }
            }
        } else if !arg.starts_with('-') {
            positional.push(args[i].clone());
        }
        i += 1;
    }
    if result.root.is_none()
        && positional.first().is_some_and(|v| {
            ["scan", "metrics", "serve", "ast"]
                .iter()
                .any(|cmd| v == cmd)
        })
    {
        result.root = positional.get(1).map(PathBuf::from);
    }
    if result.config.is_none()
        && positional.first().is_some_and(|v| v == "config")
        && positional
            .get(1)
            .is_some_and(|v| v == "check" || v == "show")
    {
        result.config = Some("analyzer.yaml".into());
    }
    result
}
fn platform() -> Result<&'static str, Error> {
    match env!("CODEFACTOR_ENGINE_TARGET") {
        "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl" => Ok("linux-x64"),
        "aarch64-unknown-linux-gnu" | "aarch64-unknown-linux-musl" => Ok("linux-arm64"),
        "x86_64-apple-darwin" => Ok("darwin-x64"),
        "aarch64-apple-darwin" => Ok("darwin-arm64"),
        "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => Ok("win32-x64"),
        "aarch64-pc-windows-msvc" => Ok("win32-arm64"),
        _ => Err(Error::Invalid("Unsupported Codefactor platform".into())),
    }
}
fn executable() -> &'static str {
    if cfg!(windows) {
        "codefactor.exe"
    } else {
        "codefactor"
    }
}
fn release_config() -> Value {
    serde_json::from_str(include_str!("../release.json")).expect("release config")
}
fn invalid(message: impl ToString) -> Error {
    Error::Invalid(message.to_string())
}
fn config_error(message: impl ToString) -> Error {
    Error::Process {
        exit_code: Some(3),
        stderr: message.to_string(),
    }
}
fn absolute(path: &Path) -> Result<PathBuf, Error> {
    let path = env::current_dir()?.join(path);
    Ok(path.canonicalize().unwrap_or(path))
}
fn selection(context: &Context) -> Result<(String, PathBuf), Error> {
    let root = absolute(context.root.as_deref().unwrap_or(Path::new(".")))?;
    let directory = if root.is_file() {
        root.parent().unwrap().to_owned()
    } else {
        root
    };
    let config = match &context.config {
        Some(file) => Some(absolute(file)?),
        None => ["analyzer.yaml", "analyzer.yml"]
            .iter()
            .map(|n| directory.join(n))
            .find(|p| p.is_file()),
    };
    let config = config.map(|file| file.canonicalize().unwrap_or(file));
    let mut version = env::var("CODEFACTOR_CORE_VERSION").ok();
    if let Some(file) = &config {
        let document: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(file).map_err(config_error)?)
                .map_err(config_error)?;
        if !document.is_null() && !document.is_mapping() {
            return Err(config_error("Codefactor config must be a mapping"));
        }
        if version.is_none() {
            if let Some(core) = document.get("core") {
                if !core.is_mapping() {
                    return Err(config_error("core must be a mapping"));
                }
                if let Some(value) = core.get("version") {
                    version = Some(
                        value
                            .as_str()
                            .ok_or_else(|| config_error("core.version must be latest or X.Y.Z"))?
                            .into(),
                    );
                }
            }
        }
    }
    let version = version.unwrap_or_else(|| "latest".into());
    if version != "latest" && !release::valid_version(&version) {
        return Err(config_error("core.version must be latest or X.Y.Z"));
    }
    let release = release_config();
    let override_base = env::var("CODEFACTOR_RELEASE_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let base = override_base
        .as_deref()
        .unwrap_or(release["base_url"].as_str().unwrap());
    let latest = if override_base.is_some() {
        ""
    } else {
        release["latest_url"].as_str().unwrap()
    };
    let source = format!(
        "{:x}",
        Sha256::digest(format!(
            "{}\n{}",
            base.trim_end_matches('/'),
            latest.trim_end_matches('/')
        ))
    );
    let project = config
        .as_ref()
        .map(|p| p.parent().unwrap())
        .unwrap_or(&directory);
    Ok((
        version,
        project
            .join(".codefactor/core")
            .join(source)
            .join(platform()?),
    ))
}
fn validate(manifest: &Value, requested: &str) -> Result<(), Error> {
    let version = manifest["version"].as_str().unwrap_or_default();
    let filename = format!(
        "codefactor-{}{}",
        platform()?,
        if cfg!(windows) { ".exe" } else { "" }
    );
    if !release::valid_version(version)
        || (requested != "latest" && version != requested)
        || manifest["platform"] != platform()?
        || manifest["file"] != filename
        || !manifest["size"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= 128 * 1024 * 1024)
        || !manifest["sha256"].as_str().is_some_and(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
    {
        return Err(invalid("Invalid Codefactor release manifest"));
    }
    Ok(())
}
fn cached_path(directory: &Path, manifest: &Value) -> PathBuf {
    directory
        .join(manifest["version"].as_str().unwrap())
        .join(manifest["sha256"].as_str().unwrap())
        .join(executable())
}
fn verified(binary: &Path, manifest: &Value) -> Result<bool, Error> {
    Ok(binary.is_file()
        && Some(binary.metadata()?.len()) == manifest["size"].as_u64()
        && manifest["sha256"] == format!("{:x}", Sha256::digest(fs::read(binary)?)))
}
pub fn resolve(context: &Context) -> Result<PathBuf, Error> {
    if let Some(binary) = env::var_os("CODEFACTOR_BINARY").filter(|v| !v.is_empty()) {
        return absolute(Path::new(&binary));
    }
    let (version, directory) = selection(context)?;
    let active = directory.join("active.json");
    if active.is_file() {
        let manifest: Value = serde_json::from_slice(&fs::read(active)?)?;
        validate(&manifest, "latest")?;
        if version == "latest" || manifest["version"] == version {
            let binary = cached_path(&directory, &manifest);
            if !verified(&binary, &manifest)? {
                return Err(invalid(
                    "Codefactor cached binary is damaged; run codefactor core update",
                ));
            }
            return Ok(binary);
        }
    }
    if version != "latest" && directory.join(&version).is_dir() {
        let mut hashes = fs::read_dir(directory.join(&version))?.collect::<Result<Vec<_>, _>>()?;
        hashes.sort_by_key(|entry| entry.file_name());
        for entry in hashes {
            let metadata = entry.path().join("codefactor.manifest.json");
            if !metadata.is_file() {
                continue;
            }
            let manifest: Value = serde_json::from_slice(&fs::read(metadata)?)?;
            validate(&manifest, &version)?;
            let binary = cached_path(&directory, &manifest);
            if entry.file_name() == manifest["sha256"].as_str().unwrap()
                && verified(&binary, &manifest)?
            {
                return Ok(binary);
            }
        }
    }
    if version == "latest" || version == env!("CODEFACTOR_ENGINE_VERSION") {
        return crate::bundled_binary();
    }
    Err(invalid(format!(
        "Codefactor Core {version} is not installed; run codefactor core update{}",
        context
            .config
            .as_ref()
            .map(|p| format!(" --config {}", p.display()))
            .unwrap_or_default()
    )))
}
struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
fn atomic_write(path: &Path, bytes: &[u8], executable: bool) -> Result<(), Error> {
    let temporary = path.with_file_name(format!(
        ".temporary-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &temporary,
                fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
            )?;
        }
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn update(config: Option<PathBuf>) -> Result<(String, PathBuf), Error> {
    if let Some(binary) = env::var_os("CODEFACTOR_BINARY").filter(|v| !v.is_empty()) {
        let binary = absolute(Path::new(&binary))?;
        if !binary.is_file() {
            return Err(invalid("CODEFACTOR_BINARY does not point to a file"));
        }
        return Ok(("external".into(), binary));
    }
    let (version, directory) = selection(&Context { config, root: None })?;
    fs::create_dir_all(&directory)?;
    let lock_path = directory.join(".update.lock");
    let mut acquired = false;
    for _ in 0..300 {
        match fs::create_dir(&lock_path) {
            Ok(()) => {
                acquired = true;
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e.into()),
        }
    }
    if !acquired {
        return Err(invalid(format!(
            "Codefactor update is locked; if no update is running remove {}",
            lock_path.display()
        )));
    }
    let _lock = Lock(lock_path);
    let release = release_config();
    let base = release::release_base(&release, &version).map_err(invalid)?;
    let bytes = release::download(&format!("{base}/manifest-{}.json", platform()?), 65536)
        .map_err(invalid)?;
    let manifest: Value = serde_json::from_slice(&bytes)?;
    validate(&manifest, &version)?;
    let binary = cached_path(&directory, &manifest);
    fs::create_dir_all(binary.parent().unwrap())?;
    if !verified(&binary, &manifest)? {
        let base = release::release_base(&release, manifest["version"].as_str().unwrap())
            .map_err(invalid)?;
        let bytes = release::download(
            &format!("{base}/{}", manifest["file"].as_str().unwrap()),
            128 * 1024 * 1024,
        )
        .map_err(invalid)?;
        if manifest["size"].as_u64() != Some(bytes.len() as u64)
            || manifest["sha256"] != format!("{:x}", Sha256::digest(&bytes))
        {
            return Err(invalid("Codefactor binary checksum mismatch"));
        }
        atomic_write(&binary, &bytes, true)?;
    }
    atomic_write(
        &binary.with_file_name("codefactor.manifest.json"),
        &bytes,
        false,
    )?;
    atomic_write(&directory.join("active.json"), &bytes, false)?;
    Ok((manifest["version"].as_str().unwrap().into(), binary))
}
/// Handle the package launcher command; deliberately not an update API.
pub fn command(args: &[OsString]) -> Result<(), Error> {
    if args.iter().any(|v| v == "--help" || v == "-h") {
        println!("Usage: codefactor core update [--config PATH]");
        return Ok(());
    }
    if args.first().is_none_or(|v| v != "update") {
        return Err(invalid("Usage: codefactor core update [--config PATH]"));
    }
    let mut config = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            i += 1;
            config = Some(PathBuf::from(&args[i]));
        } else if let Some(value) = args[i]
            .to_str()
            .and_then(|v| v.strip_prefix("--config="))
            .filter(|v| !v.is_empty())
        {
            config = Some(value.into());
        } else {
            return Err(invalid("Usage: codefactor core update [--config PATH]"));
        }
        i += 1;
    }
    let (version, binary) = update(config)?;
    println!("Codefactor Core {version}: {}", binary.display());
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn analysis_api_uses_operation_config_and_project_cache() {
        if env::var_os("CODEFACTOR_BINARY").is_some()
            || env::var_os("CODEFACTOR_CORE_VERSION").is_some()
        {
            return;
        }
        let root = env::temp_dir().join(format!(
            "codefactor-api-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("chosen.yaml");
        fs::write(&config, "core: {version: '9.8.7'}\n").unwrap();
        let (_, directory) = selection(&Context {
            config: Some(config.clone()),
            root: None,
        })
        .unwrap();
        let bytes = b"#!/bin/sh\necho '{\"schema_version\":\"2\",\"version\":\"9.8.7\",\"summary\":{},\"files\":[]}'\n";
        let manifest = serde_json::json!({"version":"9.8.7", "platform":platform().unwrap(), "file":format!("codefactor-{}", platform().unwrap()), "size":bytes.len(), "sha256":format!("{:x}", Sha256::digest(bytes))});
        let binary = cached_path(&directory, &manifest);
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        atomic_write(&binary, bytes, true).unwrap();
        atomic_write(
            &directory.join("active.json"),
            &serde_json::to_vec(&manifest).unwrap(),
            false,
        )
        .unwrap();
        let client = crate::Codefactor::new().unwrap();
        let result = client
            .scan(
                &root,
                &crate::ScanOptions {
                    config: Some(config),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(result.data["version"], "9.8.7");
        fs::remove_dir_all(root).unwrap();
    }
}
