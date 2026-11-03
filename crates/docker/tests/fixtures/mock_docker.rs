//! Process-isolated Docker CLI fixture; never contacts a Docker engine.
use std::{fs,io::{self,Write},path::Path,process};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg=="mock-child") { std::thread::sleep(std::time::Duration::from_secs(1)); fs::write("mock-child-finished", "done")?; return Ok(()); }
    let mut log = fs::OpenOptions::new().create(true).append(true).open("mock-commands.log")?;
    writeln!(log,"{}",args.join("\t"))?;
    if Path::new("mock-fail").exists() { eprintln!("mock Docker failure"); process::exit(1); }
    match args.first().map(String::as_str) {
        Some("compose") if args.iter().any(|arg| arg=="config") => print!("{}",fs::read_to_string("mock-config.json")?),
        Some("compose") => { if Path::new("mock-slow").exists() { let _child=process::Command::new(std::env::current_exe()?).arg("mock-child").spawn()?; std::thread::sleep(std::time::Duration::from_secs(10)); } fs::write("mock-up",args.join("\t"))?; println!("started"); }
        Some("ps") => print!("{}",fs::read_to_string("mock-ps.json")?),
        Some("inspect") => print!("{}",fs::read_to_string("mock-inspect.json")?),
        Some("start" | "stop") => {
            let source=fs::read_to_string("mock-inspect.json")?;
            let updated=if args.first().is_some_and(|arg| arg=="start") { source.replace("\"exited\"","\"running\"") } else { source.replace("\"running\"","\"exited\"") };
            fs::write("mock-inspect.json",updated)?; println!("ok");
        }
        Some("logs") => { println!("2026-11-03T09:00:00Z stdout log"); io::stdout().flush()?; eprintln!("2026-11-03T09:00:01Z stderr log"); println!("2026-11-03T09:00:02Z stdout after"); }
        Some("exec") => {
            println!("CONTAINER SHELL READY"); io::stdout().flush()?;
            let mut input=String::new();
            while io::stdin().read_line(&mut input)? > 0 { if input.trim()=="exit" { break; } print!("docker-shell:{input}"); io::stdout().flush()?; input.clear(); }
        }
        _ => { eprintln!("unexpected arguments: {args:?}"); process::exit(2); }
    }
    Ok(())
}
