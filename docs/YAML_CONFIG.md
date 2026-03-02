# GeoEngine YAML Configuration Reference

This document describes the `geoengine.yaml` schema and how each field is used by the current Rust CLI implementation.

## Root-Level Fields

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | String | Yes | -- | Worker name (used for registration and image tagging). |
| `version` | String | Yes | -- | Worker version string. |
| `description` | String | No | `null` | Human-readable worker description. |
| `command` | Object | No* | `null` | Runtime command configuration. |
| `local_dir_mounts` | Array | No | `null` | Static host-to-container mounts. |
| `plugins` | Object | No | `null` | GIS plugin registration flags. |
| `deploy` | Object | No | `null` | Deployment metadata. |

`*` `command` is optional at parse time, but `geoengine apply` requires it.

## `command` Section

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `program` | String | Yes | -- | Executable/interpreter (for example `python`, `Rscript`). |
| `script` | String | Yes | -- | Script path passed to `program`. |
| `inputs` | Array | No | `null` | Input argument definitions. |

During `geoengine apply`, `command.script` must exist and be a file.

### `command.inputs[]` Items

Each input definition maps user input `KEY=VALUE` to CLI args `--KEY VALUE` in the container command.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | String | Yes | -- | Input name (`--<name>` flag in container command). |
| `type` | String | Yes | -- | Input type (`file`, `folder`, `datetime`, `string`, `number`, `boolean`, `enum`). |
| `required` | Boolean | No | `true` | Required flag used by runtime validation for provided inputs. |
| `default` | Any | No | `null` | Metadata/default value exposed by `geoengine describe`. |
| `description` | String | No | `null` | Human-readable help text. |
| `enum_values` | Array | No | `null` | Choices metadata for `enum` inputs. |
| `readonly` | Boolean | No | `true` | For `file`/`folder` inputs: `true` for existing inputs, `false` for writable output paths. |
| `filetypes` | Array | No | `null` | For `file` inputs: accepted extensions (for example `[".tif", ".geotiff"]`). Use `[".*"]` or omit for all. |

### Supported Input Types

| Type | Runtime behavior |
|---|---|
| `file` | Mounted to `/inputs/<key>/<filename>`. |
| `folder` | Mounted to `/mnt/input_<key>`. |
| `datetime` | Passed through as a string argument. |
| `string` | Passed through as a string argument. |
| `number` | Passed through as a string argument. |
| `boolean` | Passed through as a string argument. |
| `enum` | Passed through as a string argument. |

Notes:
- Enum membership is not currently enforced by the CLI at run time.
- `filetypes` validation is currently applied only to `file` inputs with `readonly: true`.
- `default` values are exposed in `describe`, but are not auto-applied by `geoengine run`.

## `local_dir_mounts[]` Section

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `host_path` | String | Yes | -- | Host directory path. |
| `container_path` | String | Yes | -- | Container mount target path. |
| `readonly` | Boolean | No | `false` | Mount mode. |

During `geoengine apply`, every `host_path` must exist and be a directory.

## `plugins` Section

| Field | Type | Default | Description |
|---|---|---|---|
| `arcgis` | Boolean | `false` | Enable ArcGIS plugin registration. |
| `qgis` | Boolean | `false` | Enable QGIS plugin registration. |

## `deploy` Section

| Field | Type | Default | Description |
|---|---|---|---|
| `tenant_id` | String | `null` | Optional tenant identifier for deployment workflows. |

## Path Resolution Rules

- `command.script`:
  - Absolute path: used as-is for validation.
  - Relative path: resolved relative to the worker directory during `apply`.
- `local_dir_mounts[*].host_path`:
  - `./...`: resolved relative to the worker directory at run time.
  - Absolute path: used as-is.
  - Any other relative form is parsed, but should be avoided; prefer `./...` for worker-relative mounts.

## Validation Lifecycle

- YAML parse (`WorkerConfig::load`): validates required structural fields from the Rust structs.
- `geoengine apply`:
  - requires `command`
  - validates `command.script` exists and is a file
  - validates each `local_dir_mounts[*].host_path` exists and is a directory
  - saves applied config to `~/.geoengine/configs/<worker>.json`
- `geoengine run`:
  - uses the saved config from `apply` (not the raw YAML directly)
  - validates path inputs when provided, including read-only existence checks and file extension checks for read-only files

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
      readonly: true
      filetypes: [".tif", ".tiff"]
      description: "Input raster"

    - name: output_raster
      type: file
      required: true
      readonly: false
      description: "Output raster path"

    - name: model_name
      type: enum
      required: false
      default: "resnet50"
      enum_values: ["resnet50", "efficientnet", "unet"]
      description: "Model architecture"

local_dir_mounts:
  - host_path: ./data
    container_path: /data
    readonly: true
  - host_path: ./output
    container_path: /output
    readonly: false

plugins:
  arcgis: false
  qgis: true

deploy:
  tenant_id: null
```
