# GeoEngine Configuration Reference

This document describes all available parameters for the `geoengine.yaml` project configuration file.

## Configuration Files

| File | Location | Purpose |
|------|----------|---------|
| `geoengine.yaml` | Project root | Project-specific configuration |
| `~/.geoengine/settings.yaml` | User home | Global settings and registered projects |

---

## Project Configuration (`geoengine.yaml`)

### Root Level Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | String | **Yes** | — | Project name (used for image tagging) |
| `version` | String | No | `None` | Project version |
| `base_image` | String | No | `None` | Base Docker image to use |
| `build` | Object | No | `None` | Build configuration section |
| `runtime` | Object | No | `None` | Runtime configuration section |
| `scripts` | Map | No | `None` | Named scripts that can be executed |
| `gis` | Object | No | `None` | GIS integration configuration |
| `deploy` | Object | No | `None` | Cloud deployment settings |

---

### `build` Section

Configuration for Docker image building.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `dockerfile` | String | No | `"Dockerfile"` | Path to Dockerfile (relative to project root) |
| `context` | String | No | `"."` | Build context directory |
| `args` | Map | No | `None` | Build arguments passed to Docker |

**Example:**

```yaml
build:
  dockerfile: ./Dockerfile
  context: .
  args:
    PYTHON_VERSION: "3.11"
    GDAL_VERSION: "3.8.0"
```

---

### `runtime` Section

Configuration for container execution.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `gpu` | Boolean | No | `false` | Enable NVIDIA GPU passthrough (requires NVIDIA Container Toolkit) |
| `memory` | String | No | `None` | Memory limit (e.g., `"8g"`, `"512m"`) |
| `cpus` | Float | No | `None` | Number of CPUs to allocate (e.g., `4.0`) |
| `shm_size` | String | No | `None` | Shared memory size for PyTorch DataLoader (e.g., `"2g"`) |
| `mounts` | Array | No | `None` | Volume mounts configuration |
| `environment` | Map | No | `None` | Environment variables |
| `workdir` | String | No | `None` | Working directory inside container |

**Example:**

```yaml
runtime:
  gpu: true
  memory: "16g"
  cpus: 4
  shm_size: "2g"
  workdir: /workspace

  environment:
    PYTHONUNBUFFERED: "1"
    GDAL_DATA: "/usr/share/gdal"
    PROJ_LIB: "/usr/share/proj"
    CUDA_VISIBLE_DEVICES: "0"

  mounts:
    - host: ./data
      container: /data
    - host: ./models
      container: /models
      readonly: true
```

#### `runtime.mounts[]` Items

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `host` | String | **Yes** | — | Host path (supports `./` for relative paths) |
| `container` | String | **Yes** | — | Container path |
| `readonly` | Boolean | No | `false` | Mount as read-only |

---

### `scripts` Section

Named scripts that can be executed with `geoengine project run <project> <script>`.

Format: `<script_name>: <command>`

**Example:**

```yaml
scripts:
  default: python main.py
  train: python train.py --epochs 100
  predict: python predict.py
  process: Rscript process.R
```

---

### `gis` Section

Configuration for GIS tool integration (QGIS/ArcGIS).

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `tools` | Array | No | `None` | Array of GIS tools to expose |

**Example:**

```yaml
gis:
  tools:
    - name: classify
      label: "Land Cover Classification"
      description: "Deep learning-based classification"
      script: predict
      inputs:
        - name: input_raster
          type: raster
          required: true
      outputs:
        - name: output_raster
          type: raster
```

#### `gis.tools[]` Items

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | String | **Yes** | — | Tool identifier (used in API calls) |
| `label` | String | No | `None` | Display label in GIS UI |
| `description` | String | No | `None` | Tool description/help text |
| `script` | String | **Yes** | — | Script name to execute (from `scripts` section) |
| `inputs` | Array | No | `None` | Input parameters |
| `outputs` | Array | No | `None` | Output parameters |

#### `gis.tools[].inputs[]` and `gis.tools[].outputs[]` Items

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | String | **Yes** | — | Parameter name |
| `type` | String | **Yes** | — | Parameter type (see below) |
| `label` | String | No | `None` | Display label in GIS UI |
| `description` | String | No | `None` | Description/help text |
| `default` | Any | No | `None` | Default value |
| `required` | Boolean | No | `true` | Whether parameter is required |
| `choices` | Array | No | `None` | Valid options for choice parameters |

**Supported Parameter Types:**

| Type | Description |
|------|-------------|
| `raster` | Raster data (GeoTIFF, etc.) |
| `vector` | Vector data (Shapefile, GeoJSON, etc.) |
| `file` | General file type |
| `folder` | General directory
| `string` | Text value |
| `int` | Integer value |
| `float` | Floating-point value |
| `bool` | Boolean value |

---

### `deploy` Section

Configuration for cloud deployment (GCP).

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `gcp_project` | String | No | `None` | GCP project ID |
| `region` | String | No | `None` | GCP region (e.g., `"us-central1"`) |
| `repository` | String | No | `None` | Artifact Registry repository name |

**Example:**

```yaml
deploy:
  gcp_project: my-gcp-project
  region: us-central1
  repository: geoengine-images
```

---

## Global Settings (`~/.geoengine/settings.yaml`)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `projects` | Map | `{}` | Registered projects (name → path mapping) |
| `gcp_project` | String | `None` | Default GCP project ID |
| `gcp_region` | String | `None` | Default GCP region |
| `service_port` | Integer | `9876` | Service port (set when service starts) |
| `max_workers` | Integer | `4` | Max concurrent containers |

---

## Path Resolution

- **Relative paths** (starting with `./`): Resolved relative to the project directory
- **Absolute paths**: Used as-is

---

## Environment Variables Injected at Runtime

When executing jobs, GeoEngine injects the following environment variables:

| Variable | Description |
|----------|-------------|
| `GEOENGINE_INPUT_<NAME>` | Input parameters (uppercase, underscores for special chars) |
| `GEOENGINE_OUTPUT_DIR` | Output directory path (`/output`) |

---

## Complete Example

```yaml
name: land-cover-classifier
version: "1.0"
base_image: lawrencenika/nika-runtime:latest

build:
  dockerfile: ./Dockerfile
  context: .
  args:
    PYTHON_VERSION: "3.11"
    GDAL_VERSION: "3.8.0"

runtime:
  gpu: true
  memory: "16g"
  cpus: 4
  shm_size: "2g"
  workdir: /workspace

  mounts:
    - host: ./data
      container: /data
    - host: ./models
      container: /models
      readonly: true
    - host: ./output
      container: /output

  environment:
    PYTHONUNBUFFERED: "1"
    GDAL_DATA: /usr/share/gdal
    PROJ_LIB: /usr/share/proj
    CUDA_VISIBLE_DEVICES: "0"

scripts:
  default: python main.py
  train: python train.py --epochs 100 --batch-size 32
  predict: python predict.py
  preprocess: python preprocess.py --normalize

gis:
  tools:
    - name: classify_land_cover
      label: "Land Cover Classification"
      description: "Deep learning-based land cover classification"
      script: predict

      inputs:
        - name: input_raster
          type: raster
          label: "Input Satellite Image"
          required: true
        - name: model_name
          type: string
          label: "Model"
          default: "resnet50"
          choices: ["resnet50", "efficientnet", "unet"]
        - name: confidence_threshold
          type: float
          label: "Confidence Threshold"
          default: 0.5
          required: false

      outputs:
        - name: output_raster
          type: raster
          label: "Classification Result"

deploy:
  gcp_project: my-gcp-project
  region: us-central1
  repository: geoengine-images
```
