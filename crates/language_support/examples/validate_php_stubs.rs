//! Validate generated JSON catalogs before installing them in the IDE.
//! `cargo run -p nozhik-language-support --example validate_php_stubs -- php_8.5.json`
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args_os().skip(1) {
        let path = std::path::PathBuf::from(path);
        let catalog = nozhik_language_support::php_stubs::PhpVersionStubs::from_json(
            &std::fs::read_to_string(&path)?,
        )?;
        println!(
            "{}: PHP {}, {} entities, {} extensions",
            path.display(),
            catalog.version,
            catalog.entities.len(),
            catalog.extensions.len()
        );
    }
    Ok(())
}
