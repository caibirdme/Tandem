use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

fn main() -> io::Result<()> {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CARGO_MANIFEST_DIR is required",
        )
    })?);
    let workspace = manifest.parent().and_then(Path::parent).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Missing workspace directory")
    })?;
    let mut inputs = vec![
        workspace.join("Cargo.toml"),
        workspace.join("Cargo.lock"),
        workspace.join("rust-toolchain.toml"),
        manifest.join("build.rs"),
    ];
    for name in ["tandem-server", "copilot-runtime", "tandem-protocol"] {
        let directory = workspace.join("crates").join(name);
        inputs.push(directory.join("Cargo.toml"));
        let source = directory.join("src");
        // Watching directories also notices newly added or removed modules.
        println!("cargo:rerun-if-changed={}", source.display());
        collect_sources(&source, &mut inputs)?;
    }
    inputs.sort_unstable();
    let mut identity = DefaultHasher::new();
    "tandem-server-build-v1".hash(&mut identity);
    for path in inputs {
        println!("cargo:rerun-if-changed={}", path.display());
        let relative = path
            .strip_prefix(workspace)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        relative.hash(&mut identity);
        fs::read(&path)?.hash(&mut identity);
    }
    for name in ["TARGET", "PROFILE", "CARGO_ENCODED_RUSTFLAGS"] {
        println!("cargo:rerun-if-env-changed={name}");
        name.hash(&mut identity);
        env::var_os(name).hash(&mut identity);
    }
    // This identifies a build, not its installation inode or deployment path.
    // Re-uploading the same artifact must reconnect to the existing daemon.
    println!(
        "cargo:rustc-env=TANDEM_SERVER_BUILD_ID={:016x}",
        identity.finish()
    );
    Ok(())
}

fn collect_sources(directory: &Path, inputs: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_sources(&path, inputs)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            inputs.push(path);
        }
    }
    Ok(())
}
