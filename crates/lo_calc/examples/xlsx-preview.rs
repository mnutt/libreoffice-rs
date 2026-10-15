//! Corpus-only entry point; production callers must enforce their own sandbox.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: xlsx-preview INPUT.xlsx OUTPUT.pdf".into());
    }
    let bytes = std::fs::read(&args[1])?;
    let workbook = lo_calc::from_xlsx_bytes("", &bytes)?;
    std::fs::write(&args[2], lo_calc::to_pdf(&workbook))?;
    Ok(())
}
