fn main() {
    if let Err(err) = ai_tester::cli::main_entry() {
        eprintln!("{err}");
        std::process::exit(2);
    }
}
