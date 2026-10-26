//! Embed the Explorer icon in Windows executables, including cross builds.
use std::{env, error::Error, fs, path::PathBuf, process::Command};

fn msvc_resource_compiler() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(compiler) = env::var_os("RC") {
        return Ok(compiler.into());
    }
    if let Some(paths) = env::var_os("PATH") {
        for directory in env::split_paths(&paths) {
            let compiler = directory.join("rc.exe");
            if compiler.is_file() {
                return Ok(compiler);
            }
        }
    }
    // Cargo can find MSVC without a developer shell; locate the SDK tool too.
    let program_files =
        env::var_os("ProgramFiles(x86)").ok_or("Windows SDK location is missing")?;
    let sdk = PathBuf::from(program_files).join("Windows Kits/10/bin");
    let mut versions = fs::read_dir(sdk)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    versions.sort();
    versions
        .into_iter()
        .rev()
        .map(|path| path.join("x64/rc.exe"))
        .find(|path| path.is_file())
        .ok_or_else(|| "Cannot find Windows SDK rc.exe; set RC to its path".into())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=assets/nozhik.ico");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS")? != "windows" {
        return Ok(());
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is missing")?);
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("Manifest path is missing")?);
    let icon = root.join("assets/nozhik.ico");
    let resource = out.join("nozhik.rc");
    // Resource compilers accept forward slashes on both Windows and Unix hosts.
    fs::write(
        &resource,
        format!("1 ICON \"{}\"\n", icon.to_string_lossy().replace('\\', "/")),
    )?;
    let msvc = env::var("CARGO_CFG_TARGET_ENV")? == "msvc";
    let output = out.join(if msvc { "nozhik.res" } else { "nozhik.o" });
    let status = if msvc {
        Command::new(msvc_resource_compiler()?)
            .arg("/nologo")
            .arg("/fo")
            .arg(&output)
            .arg(&resource)
            .status()?
    } else {
        let default = if env::var("HOST")? == env::var("TARGET")? {
            "windres"
        } else {
            "x86_64-w64-mingw32-windres"
        };
        Command::new(env::var_os("WINDRES").unwrap_or_else(|| default.into()))
            .args(["--input-format=rc", "--output-format=coff"])
            .arg("--input")
            .arg(&resource)
            .arg("--output")
            .arg(&output)
            .status()?
    };
    if !status.success() {
        return Err("Windows icon resource compilation failed".into());
    }
    println!("cargo:rustc-link-arg={}", output.display());
    Ok(())
}
