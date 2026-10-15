//! Corpus entry point; callers must enforce process isolation and resource limits.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 { return Err("usage: pptx-preview INPUT.pptx OUTPUT.pdf".into()); }
    let deck = lo_impress::from_pptx_bytes("", &std::fs::read(&args[1])?)?;
    std::fs::write(&args[2], lo_impress::to_pdf(&deck))?;
    Ok(())
}
