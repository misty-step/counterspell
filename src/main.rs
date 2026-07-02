fn main() {
    if let Err(error) = counterspell::run_from_args() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
