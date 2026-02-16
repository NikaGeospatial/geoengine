# GeoEngine

Docker-based isolated runtime manager for geospatial workloads with GPU support and ArcGIS Pro/QGIS integration.

## Features

- **Isolated Execution**: Run Python/R scripts in Docker containers with GDAL, PyTorch, and other geospatial libraries
- **GPU Support**: NVIDIA GPU passthrough for CUDA-accelerated processing
- **GIS Integration**: Native plugins for ArcGIS Pro and QGIS -- tools run directly via the CLI, no proxy service required
- **Air-gapped Support**: Import Docker images for systems without internet access
- **Worker Management**: Declarative YAML configuration with `apply` for registration and plugin setup, `build` for smart image rebuilds
- **Cloud Deployment**: Push images to GCP Artifact Registry

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
# Initialize a new worker (creates geoengine.yaml template)
# Name is optional, if not specified, the current directory name is used
geoengine init --name my-worker

# Edit geoengine.yaml to configure your worker
# Add your Dockerfile and scripts

# Apply: registers the worker if new, saves worker config, and prompts GIS plugin installation if not already installed
geoengine apply

# Build: builds the Docker image (detects file changes and enforces versioning)
geoengine build
```

`geoengine apply` handles worker registration and GIS plugin management. It checks whether the worker is registered (and registers it if not) and applies any changes to the `plugins` section of `geoengine.yaml`.
It saves the current configuration of the worker, and this saved configuration is used for `build` and `run`.
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
geoengine run --input input_file=/path/to/image.tif --input model=resnet50

# Run the latest dev image (built using `geoengine build --dev`)
geoengine run --dev

# Run a named worker using its latest production image
geoengine run my-worker --input input_file=/path/to/image.tif

# JSON output mode (logs go to stderr, structured result on stdout)
geoengine run my-worker --json --input input_file=/path/to/image.tif

# Pass extra arguments to the container command (after trailing --)
geoengine run my-worker --input input_file=/data.tif -- --extra-flag value
```

**Input mapping (quick):**

- Each `--input KEY=VALUE` is forwarded as `--KEY VALUE` to the worker command.
- If `VALUE` is an existing local file/folder path, GeoEngine auto-mounts it and rewrites the argument to the container path.

When using `--json`, container logs stream to stderr and a structured JSON result is printed to stdout on completion:

```json
{
  "status": "completed",
  "exit_code": 0,
  "files": []
}
```
To note, the `files` array is currently empty, but this may change in the future.

**Advanced: input mapping details**

- File inputs are mounted read-only at `/inputs/<key>/<filename>`.
- Directory inputs are mounted read-only at `/mnt/input_N/`.
- If an input value does not exist as a local path, it is passed through as a plain string value.

### Check for Changes

```bash
# Check all tracked files (geoengine.yaml, Dockerfile, command script)
geoengine diff

# Check only the YAML configuration
geoengine diff --file yaml

# Check only the Dockerfile
geoengine diff --file docker

# Check only the command script (e.g. main.py)
geoengine diff --file command
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

### Image Management

```bash
# List images
geoengine image list

# Import from tarball (air-gapped)
geoengine image import my-image.tar --tag my-image:latest

# Remove an image
geoengine image remove my-image:latest
```

### Deploy to Cloud

```bash
# Configure GCP authentication
geoengine deploy auth --project my-gcp-project

# Push image to Artifact Registry
geoengine deploy push my-image:latest \
  --project my-gcp-project \
  --region us-central1 \
  --repository geoengine
```

### Example Workers

Example workers are available in the [examples](examples) directory. Feel free to try them out by `cd`-ing into the worker directories and running `geoengine apply` followed by `geoengine build`.

- [Simple Converter](examples/converter) - Takes an input directory, converts files to a specified format, and writes results to an output directory.

## Versioning
Workers are versioned using [Semantic Versioning](https://semver.org/). GeoEngine will enforce this when building images.
The following situations will throw errors when attempting to rebuild.
1. The version number in `geoengine.yaml` is missing.
2. The version number in `geoengine.yaml` is lower than the current image version.
3. The version number in `geoengine.yaml` is invalid.

A valid SemVer version string must be of the form `MAJOR.MINOR.PATCH`, without missing any parameters.
Ensure to follow versioning rules accordingly to avoid unexpected errors!


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
    - name: input_file
      type: file
      required: true
      description: "Input satellite image to classify"

    - name: output_folder
      type: folder
      required: true
      description: "Output folder for classification results"

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

### Configuration Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Worker name (used for image tagging and registration) |
| `version` | String | No | Worker version |
| `description` | String | No | Human-readable description |
| `command` | Object | Yes | Container command configuration |
| `local_dir_mounts` | Array | No | Persistent directory mounts |
| `plugins` | Object | No | GIS plugin flags |
| `deploy` | Object | No | Cloud deployment settings |

### `command` Section

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `program` | String | Yes | Interpreter or binary (e.g., `python`, `Rscript`) |
| `script` | String | Yes | Script to run (e.g., `main.py`) |
| `inputs` | Array | No | Input parameter definitions |

### `command.inputs[]` Items

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | String | Yes | -- | Parameter name (becomes `--name` flag) |
| `type` | String | Yes | -- | Parameter type (see below) |
| `required` | Boolean | No | `true` | Whether the parameter is required |
| `default` | Any | No | `null` | Default value |
| `description` | String | No | `null` | Help text |
| `enum_values` | Array | No | `null` | Valid choices (for `enum` type) |

**Supported Input Types:**

| Type | Description |
|------|-------------|
| `file` | File path (mounted read-only into container) |
| `folder` | Directory path (mounted read-only into container) |
| `string` | Text value |
| `number` | Numeric value |
| `boolean` | Boolean value |
| `enum` | Choice from `enum_values` list |
| `datetime` | Date/time value |

### `local_dir_mounts[]` Items

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `host_path` | String | Yes | -- | Host path (supports `./` for relative paths) |
| `container_path` | String | Yes | -- | Path inside the container |
| `readonly` | Boolean | No | `false` | Mount as read-only |

### `plugins` Section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `arcgis` | Boolean | `false` | Install ArcGIS Pro Python Toolbox plugin on `apply` |
| `qgis` | Boolean | `false` | Install QGIS Processing plugin on `apply` |

### `deploy` Section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tenant_id` | String | `null` | Deployment tenant identifier |

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
| `geoengine init [--name]`                                      | Create a new `geoengine.yaml` template                                                      |
| `geoengine apply <worker>`                                     | Register worker and manage GIS plugins                                                      |
| `geoengine build [--no-cache] [--dev] [--build-arg KEY=VALUE]` | Build the Docker image (with file change detection and version enforcement in non-dev mode) |
| `geoengine run <worker> --input KEY=VALUE [--json] [--dev]`    | Run a worker's command                                                                      |
| `geoengine diff [--file all\|yaml\|docker\|command]`           | Check which tracked files have changed since last apply                                     |
| `geoengine delete [--name <worker>]`                           | Delete a worker, clean up state and saved configuration                                     |
| `geoengine workers [--json] [--gis arcgis\|qgis]`              | List registered workers                                                                     |
| `geoengine describe <worker> [--json]`                         | Displays information from saved configuration file of specified worker                      |
| `geoengine image list\|import\|remove`                         | Manage Docker images                                                                        |
| `geoengine deploy auth\|push\|pull\|list`                      | GCP Artifact Registry operations                                                            |

## Building from Source

```bash
# Requires Rust 1.70+
git clone https://github.com/NikaGeospatial/geoengine
cd geoengine
cargo build --release

# Binary will be at target/release/geoengine
```

## License

MIT License - see [LICENSE](LICENSE) for details.
