# GeoEngine

A Docker-based isolated runtime manager for geospatial workloads — with GPU acceleration, GIS (ArcGIS Pro and QGIS) integration and interoperability, and AI agent support.

## Features

- **Isolated Execution**: Run Python/R scripts in Docker containers with GDAL, PyTorch, and other geospatial libraries
- **GPU-Ready**: NVIDIA GPU passthrough for CUDA-accelerated processing
- **GIS Integration**: Native plugins for ArcGIS Pro and QGIS -- tools run directly via the CLI, no proxy service required
- **Worker Management**: Declarative YAML configuration with `apply` for registration and plugin setup, `build` for smart image rebuilds
- **AI-Enabled**: Agent skills to automate the entire workflow

## Quick Start

### Installation

**Linux/macOS/WSL2 (curl):**
```bash
curl -fsSL https://raw.githubusercontent.com/NikaGeospatial/geoengine/main/install/install.sh | bash
```

**macOS (Homebrew):**
```bash
brew tap NikaGeospatial/geoengine
brew install geoengine
```

**Windows (PowerShell as Admin):**
```powershell
irm https://raw.githubusercontent.com/NikaGeospatial/geoengine/main/install/install.ps1 | iex
```

**Offline Installation:**
```bash
# Copy geoengine binary to the target machine, then:
./install.sh --local ./geoengine
```

### Prerequisites

- [Docker](https://docs.docker.com/get-docker/) (required)
- [NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/install-guide.html) (for GPU support)

## Usage

### Create and Apply a Worker

```bash
# Initialize a new worker (creates geoengine.yaml and pixi.toml templates)
# Name is optional, if not specified, the current directory name is used
geoengine init --name my-worker

# Use --env to select the environment type: "py" (default) or "r"
geoengine init --name my-worker --env r

# Edit geoengine.yaml and pixi.toml to configure your worker
# Add your scripts

# Apply: registers the worker if new, saves worker config, generates Dockerfile, and prompts GIS plugin installation if not already installed
geoengine apply

# Build: builds the Docker image (detects file changes and enforces versioning)
geoengine build
```

`geoengine apply` handles worker registration and GIS plugin management. It checks whether the worker is registered (and registers it if not) and applies any changes to the `plugins` section of `geoengine.yaml`.
It saves the current configuration of the worker, and this saved configuration is used for `build` and `run`.
If no Dockerfile is present, `apply` will generate one automatically.
Hence, please make sure to apply your changes **before** building or running a worker.

`geoengine build` handles Docker image builds with smart change detection:
- If build-related files (Dockerfile, geoengine.yaml build fields, command script) **and** the version number have both changed, the image is rebuilt.
- If only the version changed but no files changed, the build is skipped with a notice.
- If files changed but the version was not incremented, the build is rejected — bump the version in `geoengine.yaml` first (non-dev mode only).
- Use `--no-cache` to bypass change detection and force a full rebuild.

### Build a Docker Image

```bash
# Build from the applied configuration
geoengine build

# Build without cache (bypasses change detection)
geoengine build --no-cache

# Pass build arguments
geoengine build --build-arg PYTHON_VERSION=3.11

# Build as a dev image
geoengine build --dev
```

### Run a Worker

Each worker defines a command in `geoengine.yaml`. Input parameters are passed as `--input KEY=VALUE` flags, which are forwarded to the container script as `--KEY VALUE` arguments.
The command is run in the current directory by default. It runs the latest production image if `--dev` is not specified.

```bash
# Run the worker defined in the current directory using latest production image
geoengine run --input input-file=/path/to/image.tif --input model=resnet50

# Run the latest dev image (built using `geoengine build --dev`)
geoengine run --dev --input input-file=/path/to/image.tif --input model=resnet50

# Run a named worker using its latest production image
geoengine run my-worker --input input-file=/path/to/image.tif

# JSON output mode (logs go to stderr, structured result on stdout)
geoengine run my-worker --json --input input-file=/path/to/image.tif

# Pass extra arguments to the container command (after trailing --)
geoengine run my-worker --input input-file=/data.tif -- --extra-flag value
```

**Input mapping (quick):**

- Each `--input KEY=VALUE` is forwarded as `--KEY VALUE` to the worker command.
- If `VALUE` is an existing local file/folder path, GeoEngine auto-mounts it and rewrites the argument to the container path.

When using `--json`, container logs stream to stderr and a structured JSON result is printed to stdout on completion:

```json
{
  "status": "completed",
  "exit_code": 0,
  "files": [
    {
      "name": "output.geojson",
      "path": "/path/to/output/file",
      "size": 123,
      "kind": "output"
    }
  ]
}
```
Files array outputs the `name`, `path`, `size` (in bytes) and `kind` (either "input" or "output"). `kind` tells the GIS if
the file is part of the input or the output, as input files are also passed into GIS so that they can be displayed.

**Advanced: input mapping details**

- File inputs are mounted at `/inputs/<key>/<filename>`.
- Directory inputs are mounted at `/mnt/input_<key>/`.
- If an input value does not exist as a local path, it is passed through as a plain string value.

### Check for Changes

```bash
# Check all tracked files (geoengine.yaml, Dockerfile, command script)
geoengine diff

# Check only the geoengine.yaml configuration
geoengine diff --file config

# Check only the Dockerfile
geoengine diff --file dockerfile

# Check only the worker directory
geoengine diff --file worker

# Equivalent to no --file flag
geoengine diff --file all
```

### Manage Workers

```bash
# List registered workers
geoengine workers

# List as JSON (for programmatic use)
geoengine workers --json

# List workers registered in ArcGIS plugin (for programmatic use)
geoengine workers --gis arcgis

# Describes a worker's name, version, and parameters (defaults to current directory if worker name is not specified)
geoengine describe my-worker

# Delete a worker (removes registration, saved config and state)
geoengine delete --name my-worker

# Delete the worker in the current directory
geoengine delete
```

### Patch GeoEngine Artifacts

`geoengine patch` performs a maintenance sweep across all GeoEngine-managed artifacts. Run it after upgrading GeoEngine to bring existing workers, plugins, and agent skills up to date automatically.

```bash
geoengine patch
```

It checks and repairs the following:
- Global artifacts
- Worker artifacts
  - Worker path — warns if the registered path no longer exists on disk
  - `geoengine.yaml` — validates schema (read-only, never modified)
  - `pixi.toml` — warns if missing (read-only, never modified)
  - `Dockerfile` and `.dockerignore` — compares content against the current canonical template; silently regenerates if stale or missing
- GIS plugins
- Agent skills — syncs the GeoEngine skills from the local `skills/` directory into each installed agent's skills folder (`~/.claude/skills` for Claude, `~/.codex/skills` for Codex). Skills are compared by SHA-256 hash: changed or missing skills are updated, identical skills are skipped. Agents not installed on the machine are silently skipped.

The command exits with a non-zero status if any validation issue is found (parse errors, missing paths, reinstall failures), making it safe to use in scripts.

### Update GeoEngine

`geoengine update` updates GeoEngine to the latest version using the same installation method that was originally used (Homebrew on macOS if applicable, otherwise the curl install script on Linux/macOS/WSL2, or the PowerShell script on Windows). After a successful update it automatically runs `geoengine patch` to bring all workers, GIS plugins, and agent skills in sync with the new binary.

```bash
geoengine update
```

### Image Management

```bash
# List images
geoengine image list

# Import from tarball (air-gapped)
geoengine image import my-image.tar --tag my-image:latest

# Remove an image
geoengine image remove my-image:latest
```

### Global Environment Variables

These environment variables are injected into all workers.
```bash
# Add a global GeoEngine environment variable
geoengine env set VARIABLE=value
geoengine env set VARIABLE="val ue"
geoengine env set VAR1=val1 VAR2=val2 ...

# Set a list of GeoEngine environment variables from a file
geoengine env set -f path/to/.env

# Delete a global GeoEngine environment variable
geoengine env unset VARIABLE
geoengine env unset VAR1 VAR2 ...

# List all global environment variables
geoengine env list

# Show the value of a global environment variable
geoengine env show VARIABLE
```

If you choose to set variables from a file, make sure it follows the [`.env` format](https://hexdocs.pm/dotenvy/dotenv-file-format.html).

If you do `geoengine env set VAR1=val1 VAR2=val2 -f path/to/.env`, inline variable settings will override those in the path.
e.g. if `VAR1` is set in the file as well, the final value will be `val1`.

### Example Workers

Example workers are available in the [examples](examples) directory. Feel free to try them out by `cd`-ing into the worker directories and running `geoengine apply` followed by `geoengine build`.

- [NDVI Calculator](examples/ndvi-calculator) - Computes NDVI from multispectral satellite imagery using R.
- [Synthetic Hotspot Analysis](examples/synthetic-hotspot-analysis) - Spatial hotspot analysis from study area, incidents, and facilities layers.
- [COG Converter](examples/test-convert-cog) - Converts GeoTIFF to Cloud Optimized GeoTIFF.

## Versioning
Workers are versioned using [Semantic Versioning](https://semver.org/). GeoEngine will enforce this when building images.
The following situations will throw errors when attempting to rebuild.
1. The version number in `geoengine.yaml` is missing.
2. The version number in `geoengine.yaml` is lower than the current image version.
3. The version number in `geoengine.yaml` is invalid.

A valid SemVer version string must include all three components: `MAJOR.MINOR.PATCH`.
Follow versioning rules to avoid unexpected errors.


## Worker Configuration

Create a `geoengine.yaml` in your worker directory (or run `geoengine init` to generate a template):

```yaml
name: land-cover-classifier
version: "1.0.0"
description: "Deep learning-based land cover classification from satellite imagery"

command:
  program: python
  script: main.py
  inputs:
    - name: input-file
      type: file
      required: true
      description: "Input satellite image to classify"
      filetypes:
        - .tif
        - .tiff

    - name: output-folder
      type: folder
      required: true
      description: "Output folder for classification results"
      readonly: false

    - name: model
      type: enum
      required: false
      default: "resnet50"
      description: "Classification model to use"
      enum_values:
        - resnet50
        - efficientnet
        - unet

    - name: confidence
      type: number
      required: false
      default: 0.75
      description: "Minimum confidence threshold"

    - name: verbose
      type: boolean
      required: false
      default: false
      description: "Enable verbose output"

    - name: timestamp
      type: datetime
      required: false
      description: "Acquisition date of the input imagery"

local_dir_mounts:
  - host_path: ./data
    container_path: /data
    readonly: false
  - host_path: ./models
    container_path: /models
    readonly: true

plugins:
  arcgis: false
  qgis: true

deploy:
  tenant_id: null
```

Dependencies are declared in a `pixi.toml` file alongside the `geoengine.yaml`. Running `geoengine init` generates both files. The `pixi.toml` is used during `geoengine build` to create the conda/PyPI environment inside the Docker image.

Refer to the [Pixi manifest documentation](https://pixi.prefix.dev/latest/reference/pixi_manifest/) for more information on filling in the `pixi.toml` file.

### Configuration Reference

Refer to the [YAML configuration documentation](docs/YAML_CONFIG.md) for a complete list of configuration file options.

## GIS Plugin Architecture

```
┌─────────────────┐     ┌─────────────────┐
│   ArcGIS Pro    │     │      QGIS       │
│   (Toolbox)     │     │   (Processing)  │
└────────┬────────┘     └────────┬────────┘
         │                       │
         │   subprocess (CLI)    │
         └──────────┬────────────┘
                    │
         ┌──────────▼──────────┐
         │   geoengine CLI     │
         │       (run)         │
         └──────────┬──────────┘
                    │
                    ▼
              ┌───────────┐
              │  Docker   │
              │ Container │
              └───────────┘
```

How it works:
- Plugins are installed by `geoengine apply` based on the `plugins` section in `geoengine.yaml` if not already installed
- Run `geoengine patch` after a GeoEngine upgrade to automatically reinstall any stale plugin files
- **Discovery**: Plugins call `geoengine workers --json` to list workers, then `geoengine describe <worker> --json` to get each worker's parameter definitions as JSON
- **Execution**: Plugins invoke `geoengine run <worker> --json --input KEY=VALUE` as a subprocess
- Container logs stream to stderr in real-time (displayed as progress in the GIS UI)
- On completion, a JSON result with status and output file paths is printed to stdout
- Cancellation terminates the subprocess, which stops and removes the container

## GPU Support

### Linux

Install the NVIDIA Container Toolkit:
```bash
# Ubuntu/Debian
curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
curl -s -L https://nvidia.github.io/libnvidia-container/stable/deb/nvidia-container-toolkit.list | \
  sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' | \
  sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list
sudo apt-get update && sudo apt-get install -y nvidia-container-toolkit
sudo nvidia-ctk runtime configure --runtime=docker
sudo systemctl restart docker
```

### Windows WSL2

1. Install [NVIDIA drivers for WSL](https://developer.nvidia.com/cuda/wsl)
2. Install Docker Desktop with WSL2 backend
3. Enable GPU support in Docker Desktop settings

### macOS

CUDA is not available on macOS. PyTorch will automatically use the MPS (Metal) backend for GPU acceleration.

## CLI Reference

| Command                                                        | Description                                                                                 |
|----------------------------------------------------------------|---------------------------------------------------------------------------------------------|
| `geoengine init [--name] [--env py\|r]`                        | Create a new `geoengine.yaml` and `pixi.toml` template                                     |
| `geoengine apply [<worker>]`                                   | Register worker, generate Dockerfile if missing, save config, and manage GIS plugins        |
| `geoengine build [--no-cache] [--dev] [--build-arg KEY=VALUE]` | Build the Docker image (with file change detection and version enforcement in non-dev mode) |
| `geoengine run [<worker>] --input KEY=VALUE [--json] [--dev]`  | Run a worker's command                                                                      |
| `geoengine diff [--file all\|config\|dockerfile\|worker]`      | Check which tracked files have changed since last apply                                     |
| `geoengine delete [--name <worker>]`                           | Delete a worker, clean up state and saved configuration                                     |
| `geoengine workers [--json] [--gis arcgis\|qgis]`              | List registered workers                                                                     |
| `geoengine describe [<worker>] [--json]`                       | Displays information from saved configuration file of specified worker                      |
| `geoengine patch`                                              | Validate all artifacts, regenerate stale Dockerfiles, reinstall stale GIS plugins, and sync agent skills |
| `geoengine update`                                             | Update GeoEngine to the latest version via the original install method, then automatically run `geoengine patch` |
| `geoengine image list\|import\|remove`                         | Manage Docker images                                                                        |
| `geoengine deploy auth\|push\|pull\|list`                      | GCP Artifact Registry operations                                                            |

## AI Agents Support
AI Agents are able to assist users in automatically deploying workers from scripts. These are done using AI agent skills,
defined in the `skills` folder. They contain `SKILL.md` files with instructions on how to operate certain parts of the workflow.
In order to use AI Agents, refer to the respective sections below.

### Available Skills

| Skill | Description |
|-------|-------------|
| `use-geoengine` | Master routing skill — handles any GeoEngine request and routes to the correct sub-skill or CLI command |
| `make-geoengine-worker` | End-to-end workflow for creating a new GeoEngine worker from scratch |
| `write-geoengine-yaml` | Writes or updates the `geoengine.yaml` configuration file for a worker |
| `write-argparse` | Adds CLI argument parsing to a script so GeoEngine can pass `--name value` flags at runtime |
| `write-pixi-toml` | Writes or updates the `pixi.toml` dependency file for a worker |

### Activating Skills

#### Ensure skills directory existence

> The skills directories must exist before copying. Create them if needed:
> - **macOS/Linux:** `mkdir -p ~/.claude/skills` or `mkdir -p ~/.codex/skills`
> - **Windows:** `New-Item -ItemType Directory -Force "$env:USERPROFILE\.claude\skills"` or `New-Item -ItemType Directory -Force "$env:USERPROFILE\.codex\skills"`

#### Downloading skills from GitHub
Replace ".claude" with agent's folder. For example, if you use OpenAI's Codex, replace with ".codex".

_macOS / Linux / WSL2_
```bash
git clone --filter=blob:none --sparse https://github.com/NikaGeospatial/geoengine.git
cd geoengine
git sparse-checkout set skills
cp -r skills/* ~/.claude/skills/
```

_Windows (PowerShell)_
```powershell
git clone --filter=blob:none --sparse https://github.com/NikaGeospatial/geoengine.git
cd geoengine
git sparse-checkout set skills
Copy-Item -Recurse skills\* "$env:USERPROFILE\.claude\skills\"
```

#### Removal of geoengine folder
You can proceed to delete the `geoengine` folder that was generated from this step.

#### Checking of skills
Prompt the agent with the following prompt:
```text
What skills do you have?
```

If the agent returns a message that displays the skills shown [above](#available-skills), skills are correctly implemented.

If the agent does not show those skills available, ensure the skills are in the specific folder, and restart your agent.

### Using Skills
After copying the folder, the skills should be made available to the agent. Allow the agent access to the directory that
contains your script, then prompt the agent your needs. For example (in `examples/synthetic-hotspot-analysis`):
```text
Make the function `run_hotspot_analysis_from_files()` from `hotspot_analysis.py` a GeoEngine worker.
```
The agent will then run the whole process stipulated above automatically, even creating an argument parser if not already available
in the script.

## Development Version
Refer to the [Development Guide](docs/CONTRIBUTING.md) for instructions on setting up a development environment.

## License
MIT License - see [LICENSE](LICENSE) for details.

## Known Issues

### QGIS Plugin
- [X] QGIS readonly file inputs currently use a custom widget with a selector of layer and file. Without this, non-geometry
  files cannot be input, however the UI does look a little clunky now. Until we discover a better way to handle this, this
  will be a limitation.
- [ ] GeoEngine saves temporary files without file extensions; this can break scripts that expect an output filename to include an extension.
  This is a design decision to support temporary-file usage, so avoid relying on extensions until it's fixed.
