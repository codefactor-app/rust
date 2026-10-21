mod support;
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|v| v == "core") {
        if let Err(error) = codefactor::core::command(&args[1..]) {
            eprintln!("{error}");
            std::process::exit(
                if matches!(
                    error,
                    codefactor::Error::Process {
                        exit_code: Some(3),
                        ..
                    }
                ) {
                    3
                } else {
                    1
                },
            );
        }
        return;
    }
    let binary =
        codefactor::core::resolve(&codefactor::core::context(&args)).unwrap_or_else(|error| {
            eprintln!("Cannot start Codefactor binary: {error}");
            std::process::exit(
                if matches!(
                    error,
                    codefactor::Error::Process {
                        exit_code: Some(3),
                        ..
                    }
                ) {
                    3
                } else {
                    1
                },
            );
        });
    support::run(&binary, args);
}
