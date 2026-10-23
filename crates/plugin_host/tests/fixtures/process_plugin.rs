// Standalone std-only process fixture, compiled by the lifecycle test.
use std::io::{self, BufRead, Write};
fn main() -> io::Result<()> {
    std::fs::write("started", std::process::id().to_string())?;
    let mode = std::env::args().nth(1).unwrap_or_default();
    for line in io::stdin().lock().lines() {
        let line = line?;
        if mode == "crash" { std::process::exit(7); }
        if mode == "hang" { std::thread::sleep(std::time::Duration::from_secs(30)); }
        if mode == "oversize" { println!("{}", "x".repeat(70_000)); continue; }
        let id = line.split("\"id\":").nth(1).and_then(|s| s.split(',').next()).unwrap_or("0");
        if line.contains("executeCommand") { std::fs::write("executed", &line)?; }
        println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{}}}}");
        io::stdout().flush()?;
    }
    Ok(())
}
