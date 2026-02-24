---
name: write-geoengine-yaml
description: Editing a `geoengine.yaml` configuration file for a GeoEngine worker. This file declares what the worker runs, what inputs it accepts, how to build it, and where to mount data. This skill should run after `write-argparse` so the script already has a functioning argument parser whose flags you can reference to ensure the YAML contract is correct.
---

## 1. What Is geoengine.yaml?

GeoEngine runs user scripts inside Docker containers. The `geoengine.yaml`
file is the single source of truth that tells GeoEngine:

```
 geoengine.yaml
 +-----------+------------------------------------------+
 | name      | Worker identity (kebab-case)              |
 | version   | Semantic version                         |
 | build     | Base environment + requirements file     |
 | command   | What program/script to run + its inputs  |
 | mounts    | Extra host directories to attach         |
 | plugins   | GIS plugin registrations                 |
 | deploy    | Cloud deployment config                  |
 +-----------+------------------------------------------+
```

At runtime, `geoengine run` reads this file, translates each declared input
into a `--name value` CLI flag, and passes it to the script inside Docker.

---

## 2. Full Schema Reference

```yaml
# ── IDENTITY ──────────────────────────────────────────────────────────
name: <worker-name>              # REQUIRED. Kebab-case (e.g., ndvi-calculator)
version: "<semver>"              # Recommended. e.g., "1.0.0"
description: "<text>"            # Recommended. Human-readable summary.

# ── COMMAND ───────────────────────────────────────────────────────────
command:
  program: <interpreter>         # REQUIRED. e.g., "python", "Rscript"
  script: <script-file>          # REQUIRED. Find the file that contains the argparser.
  inputs:                        # Optional list of input parameter definitions.
    - name: <param-name>         # Becomes the --param-name CLI flag.
      type: <type>               # One of: file, folder, string, number,
                                 #         boolean, enum, datetime
      required: <bool>           # Default: true
      default: <value>           # Optional default value
      description: "<text>"      # Optional help text
      enum_values: [...]         # Required ONLY when type is "enum"
      readonly: <bool>           # Only for file/folder. Default: true

# ── MOUNTS ────────────────────────────────────────────────────────────
local_dir_mounts:                # Optional. Extra host-to-container volumes.
  - host_path: <path>            # Supports ./ for relative paths
    container_path: <path>       # Absolute container path (e.g., /data)
    readonly: <bool>             # Default: false

# ── PLUGINS ───────────────────────────────────────────────────────────
plugins:
  arcgis: <bool>                 # Default: false, only enable if user wants it
  qgis: <bool>                   # Default: false, only enable if user wants it

# ── DEPLOY ────────────────────────────────────────────────────────────
deploy:
  tenant_id: <string|null>       # Cloud tenant ID, or null
```

---

## 3. Input Types -- Detailed Reference

| Type       | What it represents          | Passed to script as               | `readonly` applies? |
|------------|-----------------------------|-----------------------------------|---------------------|
| `file`     | Single file path            | `--name /inputs/name/filename`    | Yes                 |
| `folder`   | Directory path              | `--name /mnt/input_name`          | Yes                 |
| `string`   | Free-form text              | `--name value`                    | No                  |
| `number`   | Integer or float            | `--name 42` or `--name 3.14`     | No                  |
| `boolean`  | True/false                  | `--name true` or `--name false`  | No                  |
| `enum`     | Constrained choice          | `--name choice`                   | No                  |
| `datetime` | Date/time string            | `--name 2024-01-15T10:30:00`     | No                  |

### The `readonly` field

- Applies **only** to `file` and `folder` types.
- Defaults to `true` (the mount is read-only inside the container).
- Set `readonly: false` for **output** directories/files the script writes to.
- Read-only mounts prevent the script from accidentally modifying input data.

```
 Decision tree for readonly:

   Is this input something the script READS from?
     |
     +-- YES --> readonly: true  (default, can omit)
     |
     +-- NO, the script WRITES to it
              |
              v
              readonly: false   (must be explicit)
```

### The `enum_values` field

Required **only** when `type: enum`. Lists the allowed string values:

```yaml
- name: to
  type: enum
  default: geotiff
  enum_values:
    - geotiff
    - png
    - jpeg
    - bmp
```

---

## 4. Step-by-Step Procedure

```
 START
   |
   v
 1. Read the user's script file(s)
   |
   v
 2. Identify language and interpreter
   |   Python (.py) --> program: python,  base_deps: python
   |   R      (.R)  --> program: Rscript, base_deps: r
   |
   v
 3. Read the argument parser in the script (written by write-argparse)
   |   Extract every --flag, its type, default, and choices
   |
   v
 4. Map each flag to a YAML input entry (Section 6)
   |
   v
 5. Determine readonly for each file/folder input (Section 3)
   |
   v
 6. Identify local_dir_mounts (Section 7)
   |
   v
 7. Assemble the YAML (Section 8)
   |
   v
 8. Verify the contract (Section 9)
   |
   v
 DONE
```

---

## 5. Mapping Script Arguments to YAML Inputs

Read the script's argument parser and convert each argument. Non-path inputs are straightforward to convert.
However, for path inputs, i.e. `files` and `folders` types, use your own discretion to decide if the inputs should 
be files or folders. For example:

### From Python argparse

```python
parser.add_argument("--input-dir", type=str, required=True, help="...")
```

Note that here, while `type=str`, do not get it confused as a `string` type. Read the code, and if
`--input-dir` actually takes in a path to a folder (or you can tell from the use of the word "dir" in the name),
treat it as a `folder` type. Likewise, if takes in a file path, treat it as a `file` type.
Hence, the above maps to:

```yaml
- name: input-dir
  type: folder          # infer from name or usage context
  required: true
  description: "..."
  readonly: true        # or false if the script writes to it
```

Take note that for output folders or files, since the script writes to them,
the `readonly` field **must** be set to `false`. For example,

```python
parser.add_argument("--output-dir", type=str, required=True, help="...")
```

Maps to:

```yaml
- name: output-dir
  type: folder
  required: true
  description: "..."
  readonly: false        # <-- must be set to false
```

### From R optparse

```r
make_option("--input-dir", type = "character", default = NULL, dest = "input_dir")
```

Maps to:

```yaml
- name: input-dir
  type: folder
  required: true        # default = NULL means required
  description: "..."
  readonly: true
```

### Reverse type mapping (parser --> YAML)

| Parser type          | Likely YAML type | Notes                                    |
|----------------------|------------------|------------------------------------------|
| `str` / `character`  | Depends          | Check context (see below)                |
| `int` / `integer`    | `number`         |                                          |
| `float` / `double`   | `number`         |                                          |
| `str` with `choices` | `enum`           | Carry over choices as `enum_values`      |
| `str` for bool       | `boolean`        | If default is "true"/"false"             |

**Inferring `file` vs `folder` vs `string` for `str`/`character` args:**

Look at the argument name and how it is used in the script:

| Clue                                          | YAML type  |
|-----------------------------------------------|------------|
| Name contains `-dir`, `-directory`, `-folder` | `folder`   |
| Name contains `-file`, `-path`, `-raster`     | `file`     |
| Value used with `Path()`, `open()`, `rast()`  | `file`     |
| Value used with `os.listdir()`, `list.files()`| `folder`   |
| Value is a date string                        | `datetime` |
| Everything else                               | `string`   |

---

## 6. Local Directory Mounts

`local_dir_mounts` declares **extra** host directories to attach to the
container. These are for persistent data paths lying OUTSIDE the worker's
directory that are NOT passed as `--input` flags.

```
 Do I need a local_dir_mount?

   Is this directory passed as a file/folder input parameter?
     |
     +-- YES --> Do NOT add it to local_dir_mounts.
     |           GeoEngine auto-mounts file/folder inputs.
     |
     +-- NO, it's a fixed data directory outside the worker directory that
         the script always needs.
         (e.g., model weights, reference data, shared cache)
              |
              v
              Add it to local_dir_mounts.
```

### Common patterns

```yaml
local_dir_mounts:
  # Script reads from ./data (not an input parameter)
  - host_path: ./data
    container_path: /data
    readonly: true

  # Script writes to ./output (not an input parameter)
  - host_path: ./output
    container_path: /output
    readonly: false

  # Pre-trained model weights
  - host_path: ./models
    container_path: /models
    readonly: true
```

---

## 7. Complete Examples

### Example A: Python image converter

Script: `main.py` with argparse flags `--input-dir`, `--output-dir`, `--to`

```yaml
name: converter
version: "1.0.0"
description: Batch image format converter with GeoTIFF support

command:
  program: python
  script: main.py
  inputs:
    - name: input-dir
      type: folder
      required: true
      description: Input folder containing images to convert
      readonly: true
    - name: output-dir
      type: folder
      required: true
      description: Output folder for converted images
      readonly: false
    - name: to
      type: enum
      required: true
      default: geotiff
      description: Target output format
      enum_values:
        - geotiff
        - png
        - jpeg
        - bmp

local_dir_mounts:
  - host_path: ./data
    container_path: /data
    readonly: false
  - host_path: ./output
    container_path: /output
    readonly: false

plugins:
  arcgis: false
  qgis: false

deploy:
  tenant_id: null
```

### Example B: R NDVI calculator

Script: `main.R` with optparse flags `--input-file`, `--output-dir`,
`--red-band`, `--nir-band`, `--threshold`

```yaml
name: ndvi-calculator
version: "1.0.0"
description: Computes NDVI from multispectral satellite imagery using R

command:
  program: Rscript
  script: main.R
  inputs:
    - name: input-file
      type: file
      required: true
      description: Multispectral satellite GeoTIFF (e.g., Landsat 8/9, Sentinel-2)
      readonly: true
    - name: output-dir
      type: folder
      required: true
      description: Output folder for NDVI results
      readonly: false
    - name: red-band
      type: number
      required: false
      default: 4
      description: Band index for red reflectance (default 4 for Landsat 8/9)
    - name: nir-band
      type: number
      required: false
      default: 5
      description: Band index for NIR reflectance (default 5 for Landsat 8/9)
    - name: threshold
      type: number
      required: false
      description: NDVI threshold for binary vegetation mask (e.g., 0.3)

local_dir_mounts:
  - host_path: ./data
    container_path: /data
    readonly: true
  - host_path: ./output
    container_path: /output
    readonly: false

plugins:
  arcgis: false
  qgis: false

deploy:
  tenant_id: null
```

### Example C: Minimal Python worker (no extra mounts)

```yaml
name: clip-raster
version: "1.0.0"
description: Clips a raster to a bounding box

command:
  program: python
  script: main.py
  inputs:
    - name: input-file
      type: file
      required: true
      description: Input raster to clip
      readonly: true
    - name: output-dir
      type: folder
      required: true
      description: Output directory for clipped raster
      readonly: false
    - name: bbox
      type: string
      required: true
      description: "Bounding box as 'xmin,ymin,xmax,ymax'"

plugins:
  arcgis: false
  qgis: false

deploy:
  tenant_id: null
```

---

## 8. Verification Checklist

After writing the YAML, verify every item:

- [ ] `name` is kebab-case and matches the project directory name
- [ ] `version` follows semver (e.g., "1.0.0")
- [ ] `command.program` matches the script language (`python` / `Rscript`)
- [ ] `command.script` points to the actual script filename
- [ ] Every argparse/optparse `--flag` has a matching YAML `name`
- [ ] Every YAML `name` has a matching `--flag` in the script's parser
- [ ] `type` is correct for each input (file, folder, string, number, etc.)
- [ ] `required` matches the parser (`required=True` / `default=NULL` = true)
- [ ] `default` values match between YAML and parser
- [ ] `enum_values` lists match `choices` (Python) or manual validation (R)
- [ ] `readonly: false` is set for every output directory/file
- [ ] `readonly: true` (or omitted) for every input-only directory/file
- [ ] `local_dir_mounts` does NOT duplicate auto-mounted input parameters
- [ ] `build.base_deps` matches the language (`python` or `r`)
- [ ] `build.requirements_file_path` points to the correct file (if it exists)
- [ ] `plugins` section is present (both default to false)
- [ ] `deploy.tenant_id` is set to null unless a tenant ID is known

---

## 9. Common Mistakes to Avoid

| Mistake                                    | Fix                                              |
|--------------------------------------------|--------------------------------------------------|
| Using underscores in `name` field          | Use hyphens: `my-worker`, not `my_worker`        |
| Missing `readonly: false` on output dirs   | Script will fail with permission errors           |
| Adding input params to `local_dir_mounts`  | They are auto-mounted; remove from mounts         |
| Using `base_env` instead of `base_deps`    | The field is `base_deps`                          |
| Forgetting `enum_values` for enum types    | YAML is invalid without it                        |
| Boolean default as `false` (no quotes)     | Use the bare word `false`, YAML parses it as bool |
| Mismatched names between YAML and parser   | Runtime will fail; names must match exactly        |
| Missing `build` section for R workers      | R workers need `base_deps: r`                     |
