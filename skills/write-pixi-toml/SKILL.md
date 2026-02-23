---
name: write-pixi-toml
description: Editing a `pixi.toml` file for a GeoEngine worker. Pixi is the package manager used inside the Docker container to install all dependencies (conda and PyPI). The `pixi.toml` file is consumed during `geoengine build` to create a reproducible environment. This skill should run after `write-argparse` and `write-geoengine-yaml` because the `geoengine.yaml` tells you the main script.

---

## 1. What Is pixi.toml?

Pixi is a conda-based package manager. The `pixi.toml` file declares:

- **Workspace metadata** (name, version, channels, target platforms)
- **Conda dependencies** under `[dependencies]`
- **PyPI dependencies** under `[pypi-dependencies]` (Python-only packages
  not available on conda-forge)

```
 pixi.toml structure
 +---------------------+
 | [workspace]         |  <-- name, version, channels, platforms
 +---------------------+
 | [dependencies]      |  <-- conda-forge packages (Python, R, system libs)
 +---------------------+
 | [pypi-dependencies] |  <-- pip packages (Python only, when not on conda)
 +---------------------+
```

GeoEngine's Dockerfile copies `pixi.toml` into the build stage and runs
`pixi install` to create the environment.

---

## 2. File Structure

```toml
[workspace]
name = "<worker-name>"
version = "0.1.0"
description = "A geoengine project"
channels = ["conda-forge"]
platforms = ["linux-64", "linux-aarch64"]

[dependencies]
# Conda packages go here
python = ">=3.11,<3.13"
pip = "*"

[pypi-dependencies]
# PyPI-only packages go here (Python projects only)
some-package = "*"
```

### Fixed fields

These values should **always** be set exactly as shown:

| Field                  | Value                             | Why                                    |
|------------------------|-----------------------------------|----------------------------------------|
| `channels`             | `["conda-forge"]`                 | GeoEngine uses conda-forge exclusively |
| `platforms`             | `["linux-64", "linux-aarch64"]`   | Docker targets Linux x86 and ARM       |
| `version`              | `"0.1.0"`                         | Convention for new workers             |

### Derived fields

| Field              | Source                             |
|--------------------|------------------------------------|
| `name`             | From `geoengine.yaml` `name` field |
| `description`      | From `geoengine.yaml` `description`, or `"A geoengine project"` |

---

## 3. Step-by-Step Procedure

```
 START
   |
   v
 1. Read geoengine.yaml
   |   Extract: name, description, command.program (language)
   |
   v
 2. Determine language
   |   program = "python" / "python3" --> Python project
   |   program = "Rscript"            --> R project
   |
   v
 3. Start with base template (Section 4)
   |
   v
 4. Find and read requirements file
   |   Python: requirements.txt
   |   R:      requirements.R
   |
   v
 5. Parse requirements file --> add to [dependencies] or [pypi-dependencies]
   |   (Section 5 for Python, Section 6 for R)
   |
   v
 6. Scan ALL scripts in the directory for additional imports
   |   Python: scan *.py for import/from statements
   |   R:      scan *.R for library()/require() calls
   |   (Section 7)
   |
   v
 7. Cross-reference: add any missing dependencies found in step 6
   |
   v
 8. Decide conda vs PyPI for each Python dependency (Section 8)
   |
   v
 9. Write pixi.toml. However, do NOT remove any dependencies from the base template.
   |
   v
 10. Verify (Section 11)
   |
   v
 DONE
```

---

## 4. Base Templates

### 4.1 Python base template

```toml
[workspace]
name = "<name>"
version = "0.1.0"
description = "A geoengine project"
channels = ["conda-forge"]
platforms = ["linux-64", "linux-aarch64"]

[dependencies]
pandas = ">=2.1,<3"
zarr = "*"
python = ">=3.11,<3.13"
scipy = ">=1.11,<2"
xarray = "*"
geopandas = ">=0.14,<1"
shapely = ">=2.0,<3"
fiona = ">=1.9,<2"
numpy = ">=1.26,<3"
pip = "*"
gdal = ">=3.9.0,<4"
pyproj = ">=3.6,<4"

[pypi-dependencies]
argparse = "*"
# Add Python-only packages here
```

These 12 base conda-forge dependencies and 1 PyPI dependency form GeoEngine's standard Python geospatial
environment are included in the base template. Do **not** touch these dependencies,
and do **not** include the R geospatial base packages.

### 4.2 R base template

```toml
[workspace]
name = "<name>"
version = "0.1.0"
description = "A geoengine project"
channels = ["conda-forge"]
platforms = ["linux-64", "linux-aarch64"]

[dependencies]
r-base = ">=4.3,<5"
r-recommended = "*"
r-sf = "*"
r-terra = "*"
r-stars = "*"
r-optparse = "*"
```

For R projects, the above base template is provided. Do **not** touch these dependencies,
and do **not** include the Python geospatial
base packages. Add only what the R script needs.

---

## 5. Parsing Python requirements.txt

Read `requirements.txt` line by line. Each line is a package specifier:

```
pillow>=12.1.1
rasterio>=1.3.11
numpy>=1.26.4
scikit-learn
```

### Conversion rules

```
 For each line in requirements.txt:
   |
   v
 1. Strip whitespace, skip blank lines and comments (#)
   |
   v
 2. Is this package already in the base template [dependencies]?
   |
   +-- YES --> Skip it (base template already has it with pinned versions)
   |
   +-- NO
       |
       v
 3. Is this package available on conda-forge? (Section 8)
       |
       +-- YES --> Add to [dependencies] with version "*"
       |           (or translate the pip version spec)
       |
       +-- NO  --> Add to [pypi-dependencies] with version "*"
```

### Version specifier translation

| pip format           | pixi.toml format         |
|----------------------|--------------------------|
| `package>=1.0`       | `package = ">=1.0"`      |
| `package>=1.0,<2.0`  | `package = ">=1.0,<2"`   |
| `package==1.5.0`     | `package = "==1.5.0"`    |
| `package`            | `package = "*"`          |
| `package~=1.4`       | `package = ">=1.4,<2"`   |

When in doubt, use `"*"` to let the solver pick the best version.

---

## 6. Parsing R requirements

R does NOT normally have a centralised requirements file. Instead, skip straight to scanning the
script for additional dependencies (section 7).

---

## 7. Scanning Scripts for Additional Dependencies

After parsing the requirements file, scan **every** script in the worker
directory for additional imports that may not be listed.

### 7.1 Python: scanning *.py files

Look for `import` and `from ... import` statements:

```python
import numpy as np              # --> numpy
from pathlib import Path        # --> pathlib (stdlib, skip)
import rasterio                 # --> rasterio
from PIL import Image           # --> pillow (note: import name != package name)
from sklearn.ensemble import ...# --> scikit-learn
import torch                    # --> pytorch (conda) or torch (pypi)
```

**Common import-to-package mappings (where they differ):**

| Import name     | Package name     | Where            |
|-----------------|------------------|------------------|
| `PIL`           | `pillow`         | pypi-dependencies|
| `cv2`           | `opencv`         | dependencies     |
| `sklearn`       | `scikit-learn`   | dependencies     |
| `skimage`       | `scikit-image`   | dependencies     |
| `yaml`          | `pyyaml`         | dependencies     |
| `osgeo`         | `gdal`           | dependencies     |
| `torch`         | `pytorch`        | dependencies     |
| `torchvision`   | `torchvision`    | dependencies     |
| `tf` / `tensorflow` | `tensorflow` | dependencies     |
| `geopandas`     | `geopandas`      | dependencies     |

**Skip standard library modules** (no pixi entry needed):

```
os, sys, pathlib, json, csv, re, math, datetime, collections,
itertools, functools, typing, abc, io, logging, argparse,
subprocess, shutil, tempfile, glob, uuid, hashlib, copy,
dataclasses, enum, string, textwrap, unittest, importlib
```

### 7.2 R: scanning *.R files

Look for `library()` and `require()` calls:

```r
library(terra)       # --> r-terra
library(optparse)    # --> r-optparse
library(magick)      # --> r-magick
require(sf)          # --> r-sf
```

**Skip base R packages** (no pixi entry needed):

```
base, utils, stats, graphics, grDevices, datasets, methods, tools, parallel
```

**Conversion rule**: For every `library(foo)` or `require(foo)` found,
add `r-foo = "*"` to `[dependencies]` (lowercased, hyphen-prefixed), never to
`[pypi-dependencies]`.

---

## 8. Conda vs PyPI Decision Tree (Python Only)

```
 Should this Python package go in [dependencies] or [pypi-dependencies]?
   |
   v
 Is it a geospatial/scientific C library binding?
 (gdal, rasterio, fiona, shapely, pyproj, netcdf4, h5py, scipy, numpy, etc.)
   |
   +-- YES --> [dependencies] (conda handles C deps correctly)
   |
   +-- NO
       |
       v
     Is it a common conda-forge package?
     (pandas, scikit-learn, pytorch, matplotlib, opencv, etc.)
       |
       +-- YES --> [dependencies]
       |
       +-- NO
           |
           v
         Is it a pure-Python package?
         (pillow, requests-html, flask, fastapi, pydantic, etc.)
           |
           +-- YES --> [pypi-dependencies]
           |
           +-- Unsure --> [pypi-dependencies]
               (safer default; pip can install anything)
```

### Packages that MUST be in [dependencies] (conda)

These have C/system library dependencies that conda handles:

```
gdal, rasterio, fiona, shapely, pyproj, geopandas, numpy, scipy,
pandas, netcdf4, h5py, zarr, xarray, opencv, pytorch, tensorflow,
scikit-learn, scikit-image, matplotlib, cartopy, basemap
```

### Packages that typically go in [pypi-dependencies]

Pure-Python packages or packages not on conda-forge:

```
pillow, argparse, flask, fastapi, pydantic, httpx, click,
rich, typer, loguru, tqdm (also on conda, but fine either way)
```

### When in doubt

Put it in `[pypi-dependencies]`. Pixi can install PyPI packages alongside
conda packages. This is the safer default.

---

## 9. Complete Examples

### Example A: Python converter

**requirements.txt:**
```
pillow>=12.1.1
rasterio>=1.3.11
numpy>=1.26.4
```

**Imports found in main.py:**
```python
import argparse       # stdlib, skip
from pathlib import Path  # stdlib, skip
import numpy as np    # already in base template
from PIL import Image # --> pillow (pypi)
import rasterio       # already in base template
```

**Resulting pixi.toml:**
```toml
[workspace]
name = "converter"
version = "0.1.0"
description = "A geoengine project"
channels = ["conda-forge"]
platforms = ["linux-64", "linux-aarch64"]

[dependencies]
pandas = ">=2.1,<3"
zarr = "*"
python = ">=3.11,<3.13"
scipy = ">=1.11,<2"
xarray = "*"
geopandas = ">=0.14,<1"
shapely = ">=2.0,<3"
fiona = ">=1.9,<2"
numpy = ">=1.26,<3"
pip = "*"
gdal = ">=3.9.0,<4"
pyproj = ">=3.6,<4"
rasterio = "*"

[pypi-dependencies]
argparse = "*"
pillow = "*"
```

### Example B: R NDVI calculator

**Imports found in main.R:**
```r
library(optparse)   # --> r-optparse
library(terra)      # already in base template
```

**Resulting pixi.toml:**
```toml
[workspace]
name = "ndvi-calculator"
version = "0.1.0"
description = "A geoengine project"
channels = ["conda-forge"]
platforms = ["linux-64", "linux-aarch64"]

[dependencies]
r-base = ">=4.3,<5"
r-recommended = "*"
r-sf = "*"
r-terra = "*"
r-stars = "*"
r-optparse = "*"
```

### Example C: R converter with magick

**Imports found in main.R:**
```r
library(optparse)   # --> r-optparse
library(terra)      # already in base template
library(magick)     # --> r-magick
```

**Resulting pixi.toml:**
```toml
[workspace]
name = "converter-r"
version = "0.1.0"
description = "A geoengine project"
channels = ["conda-forge"]
platforms = ["linux-64", "linux-aarch64"]

[dependencies]
r-base = ">=4.3,<5"
r-recommended = "*"
r-sf = "*"
r-terra = "*"
r-stars = "*"
r-magick = "*"
r-optparse = "*"
```

### Example D: Python ML worker (extra deps)

**requirements.txt:**
```
torch>=2.0
torchvision
pillow
```

**Imports found in main.py:**
```python
import torch           # --> pytorch (conda)
import torchvision     # --> torchvision (conda)
from PIL import Image  # --> pillow (pypi)
import numpy as np     # already in base
```

**Resulting pixi.toml:**
```toml
[workspace]
name = "land-classifier"
version = "0.1.0"
description = "A geoengine project"
channels = ["conda-forge"]
platforms = ["linux-64", "linux-aarch64"]

[dependencies]
pandas = ">=2.1,<3"
zarr = "*"
python = ">=3.11,<3.13"
scipy = ">=1.11,<2"
xarray = "*"
geopandas = ">=0.14,<1"
shapely = ">=2.0,<3"
fiona = ">=1.9,<2"
numpy = ">=1.26,<3"
pip = "*"
gdal = ">=3.9.0,<4"
pyproj = ">=3.6,<4"
pytorch = ">=2.0"
torchvision = "*"

[pypi-dependencies]
pillow = "*"
```

---

## 10. Dependency Version Pinning Strategy

| Situation                          | Version spec        | Example                   |
|------------------------------------|---------------------|---------------------------|
| Base template package              | Keep pinned range   | `numpy = ">=1.26,<3"`    |
| From requirements file with pin    | Translate the pin   | `pillow = ">=12.1"`      |
| From requirements file without pin | Use wildcard        | `pillow = "*"`            |
| Found by script scan only          | Use wildcard        | `r-magick = "*"`          |
| Critical system binding            | Keep pinned range   | `gdal = ">=3.9.0,<4"`    |

**Do not over-pin.** Use `"*"` unless the requirements file specifies a
version or the package is in the base template with an existing pin. Over-
pinning causes dependency solver conflicts in conda.

---

## 11. Verification Checklist

After writing pixi.toml, verify every item:

- [ ] `[workspace].name` matches `geoengine.yaml` `name`
- [ ] `channels = ["conda-forge"]` is set
- [ ] `platforms = ["linux-64", "linux-aarch64"]` is set
- [ ] Python projects include `python = ">=3.11,<3.13"` and `pip = "*"`
- [ ] R projects include `r-base = ">=4.3,<5"`
- [ ] All packages from `requirements.txt` / `requirements.R` are present
- [ ] All imports found by script scanning are present
- [ ] No standard library / base R packages were added
- [ ] Python C-library packages are in `[dependencies]`, not `[pypi-dependencies]`
- [ ] R packages use the `r-` prefix (e.g., `r-terra`, not `terra`)
- [ ] No duplicate entries between `[dependencies]` and `[pypi-dependencies]`
- [ ] Version pins from the base template are not overridden by weaker specs
- [ ] `[pypi-dependencies]` section exists (even if empty) for Python projects
- [ ] `[pypi-dependencies]` is **omitted** for R projects

---

## 12. Common Mistakes to Avoid

| Mistake                                          | Fix                                              |
|--------------------------------------------------|--------------------------------------------------|
| Putting `pillow` in `[dependencies]`             | Use `[pypi-dependencies]` (pure Python)          |
| Putting `r-terra` in `[pypi-dependencies]`       | R packages always go in `[dependencies]`         |
| Using `terra` instead of `r-terra` for R         | Always add `r-` prefix for R packages            |
| Adding `os`, `sys`, `pathlib` as dependencies    | These are Python stdlib, skip them               |
| Adding `base`, `utils`, `stats` for R            | These are base R, skip them                      |
| Missing `pip = "*"` in Python projects           | Required for PyPI dependency installation         |
| Using `torch` instead of `pytorch` in conda      | The conda package name is `pytorch`              |
| Using `PIL` instead of `pillow`                  | The package name is `pillow`, not `PIL`           |
| Forgetting `r-optparse` after write-argparse     | write-argparse adds optparse; include it here    |
| Setting platforms to `["osx-arm64"]`             | Docker targets are always `linux-64`, `linux-aarch64` |
| Omitting base geospatial deps for Python         | Always include the full 18-package base template |
