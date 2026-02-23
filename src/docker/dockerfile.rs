use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

fn generate_dockerfile_build(dockerfile: &mut File) -> anyhow::Result<()> {
    dockerfile.write("FROM ghcr.io/prefix-dev/pixi:0.41.4 AS build\n\n".as_bytes())?;

    // Sandbox build environment in subfolder
    dockerfile.write("WORKDIR /pixi\n\n".as_bytes())?;

    // Copy pixi configuration
    dockerfile.write("COPY pixi.toml ./\n\n".as_bytes())?;

    // Install base environment with GDAL
    dockerfile.write("RUN pixi install\n\n".as_bytes())?;

    // Generate shell hook for activation
    dockerfile.write("RUN pixi shell-hook -s bash > /shell-hook\n\n".as_bytes())?;

    // Create entrypoint script
    dockerfile.write("RUN echo '#!/bin/bash' > /pixi/entrypoint.sh && \\\n".as_bytes())?;
    dockerfile.write("\tcat /shell-hook >> /pixi/entrypoint.sh && \\\n".as_bytes())?;
    dockerfile.write("\techo 'exec \"$@\"' >> /pixi/entrypoint.sh\n\n".as_bytes())?;

    Ok(())
}

fn generate_dockerfile_runtime(dockerfile: &mut File, script_path: &str) -> anyhow::Result<()> {
    dockerfile.write("FROM ubuntu:24.04 AS final\n\n".as_bytes())?;

    // Install minimal runtime dependencies
    dockerfile.write("RUN apt-get update && apt-get install -y --no-install-recommends \\\n".as_bytes())?;
    dockerfile.write("\tca-certificates \\\n".as_bytes())?;
    dockerfile.write("\tcurl \\\n".as_bytes())?;
    dockerfile.write("\tgit \\\n".as_bytes())?;
    dockerfile.write("\t&& rm -rf /var/lib/apt/lists/*\n\n".as_bytes())?;

    // Copy pixi binary and base environment from build stage
    dockerfile.write("COPY --from=build /usr/local/bin/pixi /usr/local/bin/pixi\n".as_bytes())?;
    dockerfile.write("COPY --from=build /pixi/.pixi/envs/default /pixi/.pixi/envs/default\n".as_bytes())?;
    dockerfile.write("COPY --from=build --chmod=0755 /pixi/entrypoint.sh /pixi/entrypoint.sh\n".as_bytes())?;
    dockerfile.write("COPY --from=build /pixi/pixi.toml /pixi/pixi.toml\n\n".as_bytes())?;
    
    // Copy user's script
    dockerfile.write(format!("COPY {} ./\n\n", script_path).as_bytes())?;

    // Set up environment variables
    dockerfile.write("ENV PATH=\"/pixi/.pixi/envs/default/bin:${PATH}\"\n".as_bytes())?;
    dockerfile.write("ENV GDAL_DATA=\"/pixi/.pixi/envs/default/share/gdal\"\n".as_bytes())?;
    dockerfile.write("ENV PROJ_LIB=\"/pixi/.pixi/envs/default/share/proj\"\n\n".as_bytes())?;
    dockerfile.write("ENV PYTHONUNBUFFERED=\"1\"\n\n".as_bytes())?;

    // Set entrypoint for pixi to be used by default
    dockerfile.write("ENTRYPOINT [\"/pixi/entrypoint.sh\"]".as_bytes())?;

    Ok(())
}

pub fn generate_dockerfile(path: &PathBuf, script_path: &str) -> anyhow::Result<()> {
    let docker_path = Path::new(path).join("Dockerfile");
    let mut file = File::create(docker_path)?;
    // build stage
    generate_dockerfile_build(&mut file)?;
    // runtime stage
    generate_dockerfile_runtime(&mut file, script_path)?;
    Ok(())
}