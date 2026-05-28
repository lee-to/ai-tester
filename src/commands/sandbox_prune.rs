pub fn sandbox_prune_command(_yes: bool, _min_age_seconds: u64) -> anyhow::Result<i32> {
    println!("{}", crate::ui::header("ai-tester", "sandbox-prune"));
    println!(
        "  {} {}",
        crate::ui::paint("●", crate::ui::Tone::Warning),
        crate::ui::paint("not implemented", crate::ui::Tone::Strong)
    );
    println!(
        "  {}",
        crate::ui::kv("status", "no orphan sandbox scan implemented yet")
    );
    Ok(0)
}
