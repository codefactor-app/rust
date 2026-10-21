//! Cargo downloads and embeds the independently released Codefactor core binary.
//! ```no_run
//! let result = codefactor::Codefactor::new()?.scan("./src", &Default::default())?;
//! println!("{}", result.data["summary"]["functions"]);
//! # Ok::<(), codefactor::Error>(())
//! ```
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use wait_timeout::ChildExt;

const ENGINE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/codefactor-engine"));

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Codefactor analysis timed out")]
    Timeout,
    #[error("Codefactor failed ({exit_code:?}): {stderr}")]
    Process {
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("invalid Codefactor JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug)]
pub struct ResultData {
    pub data: Value,
    pub exit_code: i32,
    pub stderr: String,
}

#[derive(Default)]
pub struct ScanOptions {
    pub config: Option<PathBuf>,
    pub languages: Vec<String>,
    pub no_architecture: bool,
    pub strict_config: bool,
}

pub struct Codefactor {
    binary: PathBuf,
    timeout: Duration,
}

impl Codefactor {
    pub fn new() -> Result<Self, Error> {
        if let Some(path) = env::var_os("CODEFACTOR_BINARY") {
            return Ok(Self::with_binary(path));
        }
        let cache = env::var_os("CODEFACTOR_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_CACHE_HOME").map(|p| PathBuf::from(p).join("codefactor")))
            .or_else(|| env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("codefactor")))
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache/codefactor")))
            .ok_or_else(|| {
                Error::Invalid("Set CODEFACTOR_CACHE_DIR for engine extraction".into())
            })?;
        let directory = cache
            .join(env!("CODEFACTOR_ENGINE_VERSION"))
            .join(env!("CODEFACTOR_ENGINE_TARGET"))
            .join(env!("CODEFACTOR_ENGINE_SHA256"));
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let binary = directory.join(if cfg!(windows) {
            "codefactor.exe"
        } else {
            "codefactor"
        });
        let valid = binary.is_file()
            && format!("{:x}", Sha256::digest(fs::read(&binary)?))
                == env!("CODEFACTOR_ENGINE_SHA256");
        if !valid {
            let temporary = directory.join(format!(
                ".engine-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::write(&temporary, ENGINE)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
            }
            if let Err(error) = fs::rename(&temporary, &binary) {
                let _ = fs::remove_file(&temporary);
                if !binary.is_file()
                    || format!("{:x}", Sha256::digest(fs::read(&binary)?))
                        != env!("CODEFACTOR_ENGINE_SHA256")
                {
                    return Err(error.into());
                }
            }
        }
        Ok(Self::with_binary(binary))
    }

    pub fn with_binary(path: impl Into<PathBuf>) -> Self {
        Self {
            binary: path.into(),
            timeout: Duration::from_secs(120),
        }
    }

    /// Path to the installed engine, for commands that need the complete core CLI.
    pub fn binary_path(&self) -> &Path {
        &self.binary
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn scan(&self, root: impl AsRef<Path>, options: &ScanOptions) -> Result<ResultData, Error> {
        let mut args = vec!["scan".into(), "--format".into(), "json".into()];
        if let Some(config) = &options.config {
            args.extend(["--config".into(), absolute(config)?]);
        }
        if !options.languages.is_empty() {
            if options.languages.iter().any(|language| {
                ![
                    "php",
                    "python",
                    "rust",
                    "javascript",
                    "typescript",
                    "js",
                    "ts",
                ]
                .contains(&language.as_str())
            }) {
                return Err(Error::Invalid("Invalid languages".into()));
            }
            args.extend(["--lang".into(), options.languages.join(",").into()]);
        }
        if options.no_architecture {
            args.push("--no-architecture".into());
        }
        if options.strict_config {
            args.push("--strict-config".into());
        }
        args.extend(["--".into(), absolute(root.as_ref())?]);
        let result = self.run(args)?;
        if !result.data["summary"].is_object()
            || !result.data["files"].is_array()
            || !result.data["schema_version"].is_string()
        {
            return Err(Error::Invalid("Invalid Codefactor analysis report".into()));
        }
        Ok(result)
    }

    pub fn ast(&self, path: impl AsRef<Path>) -> Result<ResultData, Error> {
        self.run(vec![
            "ast".into(),
            "--format".into(),
            "json".into(),
            "--".into(),
            absolute(path.as_ref())?,
        ])
    }

    pub fn graph(
        &self,
        selector: &str,
        root: impl AsRef<Path>,
        direction: &str,
        depth: Option<usize>,
    ) -> Result<ResultData, Error> {
        if !["callers", "callees", "both"].contains(&direction) {
            return Err(Error::Invalid("Invalid graph direction".into()));
        }
        let mut args = vec![
            "graph".into(),
            direction.into(),
            "--root".into(),
            absolute(root.as_ref())?,
            "--format".into(),
            "json".into(),
        ];
        if let Some(depth) = depth {
            args.extend(["--depth".into(), depth.to_string().into()]);
        }
        args.extend(["--".into(), selector.into()]);
        self.run(args)
    }

    fn run(&self, args: Vec<std::ffi::OsString>) -> Result<ResultData, Error> {
        let mut child = Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let out = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let err = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let status = match child.wait_timeout(self.timeout)? {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
                return Err(Error::Timeout);
            }
        };
        let bytes = out
            .join()
            .map_err(|_| Error::Invalid("stdout reader failed".into()))??;
        let diagnostic = String::from_utf8_lossy(
            &err.join()
                .map_err(|_| Error::Invalid("stderr reader failed".into()))??,
        )
        .into_owned();
        let code = status.code();
        if !matches!(code, Some(0..=2)) {
            return Err(Error::Process {
                exit_code: code,
                stderr: diagnostic,
            });
        }
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Process {
            exit_code: code,
            stderr: diagnostic.clone(),
        })?;
        if !data.is_object() {
            return Err(Error::Invalid(
                "Codefactor did not return a JSON object".into(),
            ));
        }
        Ok(ResultData {
            data,
            exit_code: code.unwrap(),
            stderr: diagnostic,
        })
    }
}

fn absolute(path: &Path) -> Result<std::ffi::OsString, Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::Invalid("Invalid path".into()));
    }
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    }
    .into_os_string())
}
