pub fn trend_command() -> i32 {
    print_stub(
        "trend",
        "planned for Phase 5; will read runs/*.json and show score history",
    );
    0
}

pub fn compare_command() -> i32 {
    print_stub(
        "compare",
        "planned for Phase 5; will diff two runs side-by-side",
    );
    0
}

pub fn trace_command() -> i32 {
    print_stub(
        "trace",
        "planned for Phase 5; will pretty-print a trace JSON",
    );
    0
}

fn print_stub(command: &str, message: &str) {
    println!("{}", crate::ui::header("ai-tester", command));
    println!(
        "  {} {}",
        crate::ui::paint("●", crate::ui::Tone::Warning),
        crate::ui::paint("not implemented", crate::ui::Tone::Strong)
    );
    println!("  {}", crate::ui::kv("status", message));
}
