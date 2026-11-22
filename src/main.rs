use anyhow::Result;

fn main() -> Result<()> {
    let cfg = discord_context_mcp::config::load_config()?;
    let _db = discord_context_mcp::db::open_db(&cfg.data_dir)?;
    eprintln!("layer 1 ready");
    Ok(())
}
