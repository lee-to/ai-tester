pub fn sandbox_prune_command(_yes: bool, _min_age_seconds: u64) -> anyhow::Result<i32> {
    println!("sandbox-prune: no orphan sandbox scan implemented yet.");
    Ok(0)
}
