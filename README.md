# GeoEngine

Docker-based isolated runtime manager for geospatial workloads with GPU support and ArcGIS Pro/QGIS integration.

## Features

- **Isolated Execution**: Run Python/R scripts in Docker containers with GDAL, PyTorch, and other geospatial libraries
- **GPU Support**: NVIDIA GPU passthrough for CUDA-accelerated processing
- **GIS Integration**: Native plugins for ArcGIS Pro and QGIS -- tools run directly via the CLI, no proxy service required
- **Air-gapped Support**: Import/export Docker images for systems without internet access
- **Project Management**: YAML-based project configuration with named scripts and GIS tool definitions
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

### Create a Project

```bash
# Initialize a new project
geoengine project init --name my-geospatial-project

# Edit geoengine.yaml to configure your project
# Add your Dockerfile, scripts, and data

# Register the project
geoengine project register .

# Build the Docker image
geoengine project build my-geospatial-project

# Run a script
geoengine project run my-geospatial-project train

# Run a script with arguments (requires trailing double dash)
geoengine project run my-geospatial-project predict -- --input-raster /path/to/image.tif
```

### Run GIS Tools from the CLI

GIS tools defined in `geoengine.yaml` can be run directly from the command line. Input parameters are passed as `--input KEY=VALUE` flags, and the CLI automatically maps them to script flags.

```bash
# List tools available in a project
geoengine project tools my-project

# Run a GIS tool with input parameters
geoengine project run-tool my-project classify \
  --input input_raster=/path/to/image.tif \
  --input model_name=resnet50 \
  --output-dir ./results

# Same command with JSON output (used by plugins)
geoengine project run-tool my-project classify \
  --input input_raster=/path/to/image.tif \
  --output-dir ./results \
  --json
```

**How input mapping works:**

Each `--input KEY=VALUE` is converted to a script flag `--<flag_name> <value>`:
- If the tool's input definition has a `map_to` field, that becomes the flag name
- Otherwise, the input's `name` is used as the flag name

For example, given this tool definition:
```yaml
gis:
  tools:
    - name: classify
      script: predict
      inputs:
        - name: input_raster
          type: raster
          map_to: input-file    # Maps to --input-file
        - name: model_name
          type: string          # No map_to, so maps to --model_name
```

Running:
```bash
geoengine project run-tool my-project classify \
  --input input_raster=/data/image.tif \
  --input model_name=resnet50
```

Executes the script as:
```bash
python predict.py --input-file /inputs/image.tif --model_name resnet50
```

File/directory paths are automatically:
- Mounted read-only into `/inputs/<filename>` (files) or `/mnt/input_N/` (directories)
- Rewritten in the command line to use the container path

When using `--json`, container logs stream to stderr and a structured JSON result is printed to stdout on completion:

```json
{
  "status": "completed",
  "exit_code": 0,
  "output_dir": "/absolute/path/to/results",
  "files": [
    {"name": "result.tif", "path": "/absolute/path/to/results/result.tif", "size": 12345}
  ]
}
```

### Run Containers Directly

```bash
# Run a container with GPU and mounts
geoengine run my-image:latest python train.py \
  --gpu \
  --mount ./data:/data \
  --mount ./output:/output \
  --env CUDA_VISIBLE_DEVICES=0 \
  --memory 16g

# Run interactively
geoengine run -t ubuntu:latest bash
```

### Image Management

```bash
# List images
geoengine image list

# Import from tarball (air-gapped)
geoengine image import my-image.tar --tag my-image:latest

# Export for transfer
geoengine image export my-image:latest -o my-image.tar

# Pull from registry
geoengine image pull nvidia/cuda:12.0-base
```

### GIS Integration

The QGIS and ArcGIS plugins invoke the `geoengine` CLI directly -- no proxy service is required. Install the plugins, and any registered project with GIS tools will appear automatically.

```bash
# Install the ArcGIS Pro plugin
geoengine service register arcgis

# Install the QGIS plugin
geoengine service register qgis
```

The plugins will:
1. Discover registered projects and their tools via `geoengine project list --json` and `geoengine project tools <name>`
2. Present tools in the native Processing toolbox / Python Toolbox with typed parameters
3. Execute tools via `geoengine project run-tool`, streaming real-time container output as progress messages
4. Support cancellation (QGIS) by terminating the subprocess

#### Legacy Proxy Service

A proxy HTTP service is also available for advanced use cases (remote Docker hosts, shared job queues, web UIs):

```bash
# Start the proxy service
geoengine service start

# Check service status
geoengine service status

# View running jobs
geoengine service jobs
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

### Example Project
Example projects are available in the [examples](examples) directory. Feel free to try them out by `cd`-ing
into the projects and register them as local geoengine projects.
- [Simple Converter](examples/converter) - Takes input directory, converts files in it to specified format,
  spits it out into output directory.

## Project Configuration

Create a `geoengine.yaml` in your project directory:

```yaml
name: land-cover-classifier
version: "1.0"

build:
  dockerfile: ./Dockerfile
  context: .

runtime:
  gpu: true
  memory: "16g"
  cpus: 4
  shm_size: "2g"

  mounts:
    - host: ./data
      container: /data
    - host: ./output
      container: /output

  environment:
    CUDA_VISIBLE_DEVICES: "0"
    PYTHONUNBUFFERED: "1"

  workdir: /workspace

scripts:
  default: python main.py
  train: python train.py --epochs 100
  predict: python predict.py

# GIS tools (optional)
gis:
  tools:
    - name: classify
      label: "Land Cover Classification"
      script: predict
      inputs:
        - name: input_raster
          type: raster
          label: "Input Image"
      outputs:
        - name: output_raster
          type: raster
          label: "Classification"
```

## GIS Plugin Architecture

```
┌─────────────────┐     ┌─────────────────┐
│   ArcGIS Pro    │     │      QGIS       │
│   (Toolbox)     │     │    (Plugin)     │
└────────┬────────┘     └────────┬────────┘
         │                       │
         │   subprocess (CLI)    │
         └──────────┬────────────┘
                    │
         ┌──────────▼──────────┐
         │   geoengine CLI     │
         │  (project run-tool) │
         └──────────┬──────────┘
                    │
                    ▼
              ┌───────────┐
              │  Docker   │
              │ Container │
              └───────────┘
```

How it works:
- Plugins discover tools by calling `geoengine project tools <project>` (JSON output)
- Tool execution invokes `geoengine project run-tool` as a subprocess
- Container logs stream to stderr in real-time (displayed as progress in the GIS UI)
- On completion, a JSON result with output file paths is printed to stdout
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

## API Reference

### CLI Commands

| Command | Description |
|---------|-------------|
| `geoengine project init` | Create a new `geoengine.yaml` |
| `geoengine project register <path>` | Register a project directory |
| `geoengine project list [--json]` | List registered projects |
| `geoengine project build <project>` | Build the Docker image |
| `geoengine project run <project> <script>` | Run a named script |
| `geoengine project tools <project>` | List GIS tools (JSON) |
| `geoengine project run-tool <project> <tool> --input KEY=VALUE ...` | Run a GIS tool |
| `geoengine project show <project>` | Show project configuration |
| `geoengine run <image> [command]` | Run a container directly |
| `geoengine image list\|pull\|import\|export\|remove` | Manage Docker images |
| `geoengine service start\|stop\|status\|register` | Manage the legacy proxy service |
| `geoengine deploy auth\|push\|pull\|list` | GCP Artifact Registry operations |

### REST API Endpoints (Legacy Proxy)

When the proxy service is running (`geoengine service start`):

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/health` | GET | Health check |
| `/api/jobs` | GET | List jobs |
| `/api/jobs` | POST | Submit job |
| `/api/jobs/{id}` | GET | Get job status |
| `/api/jobs/{id}` | DELETE | Cancel job |
| `/api/jobs/{id}/output` | GET | Get job outputs |
| `/api/projects` | GET | List projects |
| `/api/projects/{name}/tools` | GET | Get project tools |

### Job Submission

```json
POST /api/jobs
{
  "project": "my-project",
  "tool": "classify",
  "inputs": {
    "input_raster": "/path/to/image.tif",
    "model": "resnet50"
  },
  "output_dir": "/path/to/outputs"
}
```

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
