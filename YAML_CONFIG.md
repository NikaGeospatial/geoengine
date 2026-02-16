# GeoEngine YAML Configuration Reference

This document describes all available parameters for the `geoengine.yaml` worker configuration file.

---

## Root-Level Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | String | **Yes** | -- | Worker name |
| `version` | String | No | `null` | Version string |
| `description` | String | No | `null` | Worker description |
| `command` | Object | No | `null` | Command configuration |
| `local_dir_mounts` | Array | No | `null` | Volume mounts |
| `plugins` | Object | No | `null` | GIS plugin registration |
| `deploy` | Object | No | `null` | Deployment configuration |

---

## `command` Section

Defines the program and script the worker executes, along with any input parameters.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `program` | String | **Yes** | -- | Program to run (e.g., `python`, `node`) |
| `script` | String | **Yes** | -- | Script to execute (e.g., `main.py`) |
| `inputs` | Array | No | `null` | Input parameter definitions |

### `command.inputs[]` Items

Each input defines a CLI flag that is passed to the command at runtime. The parameter `name` becomes the `--name` flag.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | String | **Yes** | -- | Parameter name (becomes `--name` flag) |
| `type` | String | **Yes** | -- | Parameter type (see Input Types below) |
| `required` | Boolean | No | `true` | Whether the parameter is required |
| `default` | Any | No | `null` | Default value |
| `description` | String | No | `null` | Help text / description |
| `enum_values` | Array | No | `null` | Allowed values (only for `enum` type) |

### Input Types

| Type | Description |
|------|-------------|
| `file` | Path to a file on disk. Auto-mounted read-only into the container. |
| `folder` | Path to a directory. Auto-mounted into the container. |
| `datetime` | Datetime string, passed as-is. |
| `string` | Text value. |
| `number` | Numeric value (int or float). |
| `boolean` | True/false value. |
| `enum` | Constrained to values listed in `enum_values`. |

---

## `local_dir_mounts` Section

Defines host-to-container volume mounts.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `host_path` | String | **Yes** | -- | Host path (supports `./` for relative paths) |
| `container_path` | String | **Yes** | -- | Container path |
| `readonly` | Boolean | No | `false` | Mount as read-only |

---

## `plugins` Section

Controls registration of the worker as a tool in GIS applications.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `arcgis` | Boolean | No | `false` | Register with ArcGIS Pro |
| `qgis` | Boolean | No | `false` | Register with QGIS |

---

## `deploy` Section

Placeholder for future deployment configuration.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `tenant_id` | String | No | `null` | Tenant ID |

---

## Path Resolution

- **Relative paths** (starting with `./`): Resolved relative to the worker directory.
- **Absolute paths**: Used as-is.

---

## Complete Example

```yaml
name: land-cover-classifier
version: "1.0.0"
description: "Deep learning-based land cover classification worker"

command:
  program: python
  script: predict.py
  inputs:
    - name: input_raster
      type: file
      required: true
      description: "Input satellite image"
    - name: output_dir
      type: folder
      required: true
      description: "Output directory for results"
    - name: model_name
      type: enum
      default: "resnet50"
      enum_values: ["resnet50", "efficientnet", "unet"]
      description: "Model architecture to use"
    - name: confidence_threshold
      type: number
      required: false
      default: 0.5
      description: "Minimum confidence threshold"
    - name: use_gpu
      type: boolean
      default: false
      description: "Enable GPU acceleration"
    - name: timestamp
      type: datetime
      required: false
      description: "Processing timestamp"

local_dir_mounts:
  - host_path: ./data
    container_path: /data
  - host_path: ./models
    container_path: /models
    readonly: true

plugins:
  arcgis: true
  qgis: true

deploy:
  tenant_id: null
```
