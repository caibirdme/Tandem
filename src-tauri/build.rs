use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let output =
        PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Cargo OUT_DIR is required")
        })?);
    let profile = output.ancestors().nth(3).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid Cargo output directory",
        )
    })?;
    // Tauri copies resources without removing deleted targets from standalone builds.
    match fs::remove_dir_all(profile.join("remote")) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    tauri_build::build();
    Ok(())
}
