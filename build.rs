#[path = "src/release.rs"]
mod release;
use release::{download, release_base, valid_version};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=release.json");
    println!("cargo:rerun-if-changed=src/release.rs");
    println!("cargo:rerun-if-changed=certs/gitlab.local.pem");
    for name in [
        "CODEFACTOR_BINARY",
        "CODEFACTOR_CORE_VERSION",
        "CODEFACTOR_RELEASE_BASE_URL",
        "CODEFACTOR_TOKEN",
        "CODEFACTOR_JOB_TOKEN",
        "CI_JOB_TOKEN",
        "CODEFACTOR_CA_BUNDLE",
        "CODEFACTOR_OFFLINE",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let config: Value = serde_json::from_str(include_str!("release.json"))?;
    let selected = env::var("CODEFACTOR_CORE_VERSION")
        .unwrap_or_else(|_| config["version"].as_str().unwrap_or("latest").into());
    let requested = selected.as_str();
    if env::var_os("CODEFACTOR_BINARY").is_none()
        && requested != "latest"
        && !valid_version(requested)
    {
        return Err("core.version must be latest or X.Y.Z".into());
    }
    let mut version = "custom".to_owned();
    let target = env::var("TARGET")?;
    let supplied_binary = env::var_os("CODEFACTOR_BINARY");
    let platform = match target.as_str() {
        "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl" => "linux-x64",
        "aarch64-unknown-linux-gnu" | "aarch64-unknown-linux-musl" => "linux-arm64",
        "x86_64-apple-darwin" => "darwin-x64",
        "aarch64-apple-darwin" => "darwin-arm64",
        "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => "win32-x64",
        "aarch64-pc-windows-msvc" => "win32-arm64",
        _ if supplied_binary.is_some() => "custom",
        _ => {
            return Err(format!(
            "Unsupported Codefactor target {target}; supply CODEFACTOR_BINARY for a custom target"
        )
            .into())
        }
    };
    let directory = PathBuf::from(env::var_os("OUT_DIR").ok_or("Missing OUT_DIR")?);
    let binary = directory.join("codefactor-engine");
    let metadata = directory.join("codefactor.manifest.json");
    if let Some(override_path) = supplied_binary {
        println!(
            "cargo:rerun-if-changed={}",
            PathBuf::from(&override_path).display()
        );
        fs::copy(override_path, &binary)?;
    } else {
        let offline = env::var("CODEFACTOR_OFFLINE").as_deref() == Ok("1");
        let cached = if (offline || requested != "latest") && binary.is_file() && metadata.is_file()
        {
            let previous: Value = serde_json::from_slice(&fs::read(&metadata)?)?;
            let cached_version = previous["version"].as_str().unwrap_or_default();
            let valid = valid_version(cached_version)
                && (requested == "latest" || cached_version == requested)
                && previous["platform"] == platform
                && previous["size"].as_u64() == Some(binary.metadata()?.len())
                && previous["sha256"] == format!("{:x}", Sha256::digest(fs::read(&binary)?));
            if valid {
                version = cached_version.into();
            }
            valid
        } else {
            false
        };
        if offline {
            if !cached {
                return Err("Offline engine installation requires CODEFACTOR_BINARY or a cached verified engine".into());
            }
        } else if requested == "latest" || !cached {
            if requested == "latest" {
                // A missing tracked path makes Cargo check the release on each build,
                // including rebuilds that reuse this package's OUT_DIR.
                println!(
                    "cargo:rerun-if-changed={}",
                    directory.join(".check-latest-release").display()
                );
            }
            let base = release_base(&config, requested)?;
            let manifest_bytes = download(&format!("{base}/manifest-{platform}.json"), 65536)?;
            let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
            let filename = format!(
                "codefactor-{platform}{}",
                if platform.starts_with("win32") {
                    ".exe"
                } else {
                    ""
                }
            );
            let actual = manifest["version"].as_str().unwrap_or_default();
            if !valid_version(actual)
                || (requested != "latest" && actual != requested)
                || manifest["platform"] != platform
                || manifest["file"] != filename
                || !manifest["sha256"].as_str().is_some_and(|sha| {
                    sha.len() == 64
                        && sha
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            {
                return Err("Invalid Codefactor release manifest".into());
            }
            version = actual.into();
            let matches = binary.is_file()
                && manifest["size"].as_u64() == Some(binary.metadata()?.len())
                && manifest["sha256"] == format!("{:x}", Sha256::digest(fs::read(&binary)?));
            if !matches {
                let base = release_base(&config, actual)?;
                let bytes = download(&format!("{base}/{filename}"), 128 * 1024 * 1024)?;
                if manifest["size"].as_u64() != Some(bytes.len() as u64)
                    || manifest["sha256"] != format!("{:x}", Sha256::digest(&bytes))
                {
                    return Err("Codefactor binary checksum mismatch".into());
                }
                fs::write(&binary, bytes)?;
            }
            fs::write(metadata, manifest_bytes)?;
        }
    }
    println!(
        "cargo:rustc-env=CODEFACTOR_ENGINE_SHA256={:x}",
        Sha256::digest(fs::read(binary)?)
    );
    println!("cargo:rustc-env=CODEFACTOR_ENGINE_TARGET={target}");
    println!("cargo:rustc-env=CODEFACTOR_ENGINE_VERSION={version}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        // Do not include request headers or credentials in build diagnostics.
        panic!("Cannot install Codefactor engine: {error}");
    }
}
