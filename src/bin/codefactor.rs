mod support;

fn main() {
    let client = codefactor::Codefactor::new().unwrap_or_else(|error| {
        eprintln!("Cannot install Codefactor binary: {error}");
        std::process::exit(1);
    });
    support::run(client.binary_path(), std::env::args_os().skip(1));
}
