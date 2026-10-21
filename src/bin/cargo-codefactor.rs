mod support;

fn main() {
    // Cargo installs both executables together. Only codefactor embeds the engine.
    let executable = std::env::current_exe().unwrap_or_else(|error| {
        eprintln!("Cannot locate Codefactor installation: {error}");
        std::process::exit(1);
    });
    let binary = executable.with_file_name(if cfg!(windows) {
        "codefactor.exe"
    } else {
        "codefactor"
    });
    let mut arguments = std::env::args_os().skip(1).peekable();
    if arguments
        .peek()
        .is_some_and(|argument| argument == "codefactor")
    {
        arguments.next();
    }
    support::run(&binary, arguments);
}
