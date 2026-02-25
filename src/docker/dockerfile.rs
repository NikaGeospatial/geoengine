use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

fn generate_dockerfile_build(dockerfile: &mut File) -> anyhow::Result<()> {
    dockerfile.write_all(b"FROM ghcr.io/prefix-dev/pixi:0.41.4 AS build\n\n")?;

    // Sandbox build environment in subfolder
    dockerfile.write_all(b"WORKDIR /pixi\n\n")?;

    // Copy pixi configuration
    dockerfile.write_all(b"COPY pixi.toml ./\n\n")?;

    // Install base environment with GDAL
    dockerfile.write_all(b"RUN pixi install\n\n")?;

    // Generate shell hook for activation
    dockerfile.write_all(b"RUN pixi shell-hook -s bash > /shell-hook\n\n")?;

    // Create entrypoint script
    dockerfile.write_all(b"RUN echo '#!/bin/bash' > /pixi/entrypoint.sh && \\\n")?;
    dockerfile.write_all(b"\tcat /shell-hook >> /pixi/entrypoint.sh && \\\n")?;
    dockerfile.write_all(b"\techo 'exec \"$@\"' >> /pixi/entrypoint.sh\n\n")?;

    Ok(())
}

fn generate_dockerfile_runtime(dockerfile: &mut File) -> anyhow::Result<()> {
    dockerfile.write_all(b"FROM ubuntu:24.04 AS final\n\n")?;

    // Install minimal runtime dependencies
    dockerfile.write_all(b"RUN apt-get update && apt-get install -y --no-install-recommends \\\n")?;
    dockerfile.write_all(b"\tca-certificates \\\n")?;
    dockerfile.write_all(b"\tcurl \\\n")?;
    dockerfile.write_all(b"\tgit \\\n")?;
    dockerfile.write_all(b"\t&& rm -rf /var/lib/apt/lists/*\n\n")?;

    // Copy pixi binary and base environment from build stage
    dockerfile.write_all(b"COPY --from=build /pixi/.pixi/envs/default /pixi/.pixi/envs/default\n")?;
    dockerfile.write_all(b"COPY --from=build --chmod=0755 /pixi/entrypoint.sh /pixi/entrypoint.sh\n")?;
    dockerfile.write_all(b"COPY --from=build /pixi/pixi.toml /pixi/pixi.toml\n\n")?;
    
    // Copy project tree into isolated folder in container, set its working directory
    dockerfile.write_all(b"WORKDIR /app\n\n")?;
    dockerfile.write_all(b"COPY . /app\n\n")?;

    // Create user and set working directory permissions
    dockerfile.write_all(b"RUN useradd -m worker && chown -R worker:worker /app\n\n")?;

    // Set up environment variables
    dockerfile.write_all(b"ENV PATH=\"/pixi/.pixi/envs/default/bin:${PATH}\"\n")?;
    dockerfile.write_all(b"ENV GDAL_DATA=\"/pixi/.pixi/envs/default/share/gdal\"\n")?;
    dockerfile.write_all(b"ENV PROJ_LIB=\"/pixi/.pixi/envs/default/share/proj\"\n\n")?;
    dockerfile.write_all(b"ENV PYTHONUNBUFFERED=\"1\"\n\n")?;

    // Set entrypoint for pixi to be used by default
    dockerfile.write_all(b"ENTRYPOINT [\"/pixi/entrypoint.sh\"]\n")?;

    Ok(())
}

/// Ignored files when building docker image
fn generate_dockerignore(ignorefile: &mut File) -> anyhow::Result<()> {
    let ignorables = [
        "geoengine.yaml",
        "Dockerfile",
        ".venv",
        ".idea",
        ".pixi",
        "__pycache__",
        "*.pyc",
        "*.pyo",
        ".pytest_cache"
    ];
    for ignorable in ignorables.iter() {
        ignorefile.write_all(format!("{}/\n", ignorable).as_bytes())?;
    }
    Ok(())
}

pub fn generate_dockerfile(path: &PathBuf) -> anyhow::Result<()> {
    let docker_path = Path::new(path).join("Dockerfile");
    let mut dockerfile = File::create(docker_path)?;
    // build stage
    generate_dockerfile_build(&mut dockerfile)?;
    // runtime stage
    generate_dockerfile_runtime(&mut dockerfile)?;

    let mut ignorefile = File::create(path.join(".dockerignore"))?;
    generate_dockerignore(&mut ignorefile)?;
    Ok(())
}