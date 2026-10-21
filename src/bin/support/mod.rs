use std::{ffi::OsString, path::Path, process::Command};

pub fn run(binary: &Path, arguments: impl IntoIterator<Item = OsString>) -> ! {
    let mut command = Command::new(binary);
    command.args(arguments);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("Cannot start Codefactor binary: {error}");
        std::process::exit(if error.kind() == std::io::ErrorKind::NotFound {
            127
        } else {
            126
        });
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(error) => {
                eprintln!("Cannot start Codefactor binary: {error}");
                std::process::exit(if error.kind() == std::io::ErrorKind::NotFound {
                    127
                } else {
                    126
                });
            }
        }
    }
}
