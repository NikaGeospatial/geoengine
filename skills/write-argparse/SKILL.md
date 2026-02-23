---
name: write-argparse
description: Adding CLI argument parsing to a user's script so that GeoEngine can pass `--name value` flags at runtime. Do nothing if the script already has an argument parser.
---

## 1. When This Skill Runs

```
 User script (main.py / main.R)
         |
         v
 +-----------------------+
 | Does it already have  |
 | an argument parser?   |
 +----------+------------+
       YES  |  NO
        |   |   |
        v   |   v
    STOP.   | Generate argparse / optparse
    Done.   | according to main function's signature.
            +------------------------------> Insert into script
```

### Detection rules

**Python** -- the script already has an argument parser if ANY of these appear:

- `import argparse`
- `argparse.ArgumentParser`
- `add_argument(`

**R** -- the script already has an argument parser if ANY of these appear:

- `library(optparse)`
- `make_option(`
- `OptionParser(`
- `parse_args(`

If any of those patterns are found, **stop**. Do not duplicate or overwrite the
existing parser.

---

## 2. Finding the Main Function

This skill does **not** depend on `geoengine.yaml`. It reads the user's
script directly and derives the argument parser from the **main function's
signature**.

### Locating the script

Look for `main.py` or `main.R` in the project directory. If neither exists,
scan for other `.py` / `.R` files in the directory.

### Locating the main function

**Python** -- look for (in order of priority):

1. A function literally named `main()`, `run()`, or `process()`
2. A function called inside `if __name__ == "__main__":`
3. A function whose name suggests it is the entry point (e.g.,
   `run_batch()`, `calculate_ndvi()`, `convert_images()`)

**R** -- look for (in order of priority):

1. A function literally named `main()`, `run()`, or `process()`
2. A top-level function call at the bottom of the script (R scripts typically
   define functions first, then call the main one at the end)
3. The most prominent function whose name suggests it is the entry point

### If the main function is unclear -- STOP

If there is no obvious main function, or there are multiple candidates and
you cannot determine which one the user intends to expose as a tool, **stop
and ask the user**:

> "I found multiple candidate functions in your script: `function_a()`,
> `function_b()`. Which one should be the main entry point for the worker?"

Do **not** guess. Clarify before proceeding.

---

## 3. Reading the Main Function's Signature

Once you have identified the main function, extract its **parameter list**.
Each parameter becomes a CLI flag in the generated argparser.

### 3.1 Python example

```python
def calculate_ndvi(input_file, output_dir, red_band=4, nir_band=5,
                   threshold=None):
    ...
```

Extract:

| Parameter     | Has default? | Default value | Python type hint / inferred type |
|---------------|--------------|---------------|----------------------------------|
| `input_file`  | No           | --            | `str` (path)                     |
| `output_dir`  | No           | --            | `str` (path)                     |
| `red_band`    | Yes          | `4`           | `int` --> use `float` (see below)|
| `nir_band`    | Yes          | `5`           | `int` --> use `float` (see below)|
| `threshold`   | Yes          | `None`        | `float` (from context)           |

### 3.2 R example

```r
calculate_ndvi <- function(input_file, output_dir, red_band = 4L,
                           nir_band = 5L, threshold = NULL) {
  ...
}
```

Extract:

| Parameter     | Has default? | Default value | R type                     |
|---------------|--------------|---------------|----------------------------|
| `input_file`  | No           | --            | `character` (path)         |
| `output_dir`  | No           | --            | `character` (path)         |
| `red_band`    | Yes          | `4L`          | `double` (see below)       |
| `nir_band`    | Yes          | `5L`          | `double` (see below)       |
| `threshold`   | Yes          | `NULL`        | `double` (from context)    |

### 3.3 Required vs optional

- **No default** --> the parameter is required (`required=True` in Python,
  `default = NULL` + manual check in R).
- **Has a default** --> optional, carry over the default value.
- **Default is `None` / `NULL`** --> optional, no default in the CLI.

---

## 4. Type Mapping (Function Signature --> argparse / optparse)

### 4.1 Python (`argparse`)

| Inferred Python type   | `argparse` configuration                                            |
|------------------------|---------------------------------------------------------------------|
| `str` (path-like name) | `type=str` -- see Section 4.3 for path detection                   |
| `str` (general)        | `type=str`                                                          |
| `int`                  | `type=float` -- **always use float, never int** (see Section 4.4)  |
| `float`                | `type=float`                                                        |
| `bool`                 | `type=str, default="false"` -- convert after parsing (Section 5)   |
| `None` default         | `type=float` or `type=str` -- infer from how it is used in the body|

### 4.2 R (`optparse`)

| Inferred R type        | `optparse` configuration                                           |
|------------------------|---------------------------------------------------------------------|
| `character` (path)     | `type = "character"`                                                |
| `character` (general)  | `type = "character"`                                                |
| `integer` (e.g., `4L`) | `type = "double"` -- **always use double, never integer**          |
| `numeric` / `double`   | `type = "double"`                                                   |
| `logical`              | `type = "character", default = "false"` -- convert after parsing   |
| `NULL` default         | `type = "double"` or `type = "character"` -- infer from usage      |

### 4.3 Detecting path parameters

Look at the **parameter name** and **how it is used** in the function body:

| Clue in name                                  | Likely type |
|-----------------------------------------------|-------------|
| Name contains `_dir`, `_directory`, `_folder` | path (dir)  |
| Name contains `_file`, `_path`, `_raster`     | path (file) |
| Name contains `_input`, `_output`, `_src`, `_dst` | path    |

| Clue in function body                         | Likely type |
|-----------------------------------------------|-------------|
| Used with `Path()`, `open()`, `os.path.*`     | path        |
| Used with `rasterio.open()`, `Image.open()`   | path (file) |
| Used with `os.listdir()`, `os.scandir()`      | path (dir)  |
| Used with `terra::rast()`, `magick::image_read()` | path (file) |
| Used with `list.files()`, `dir.exists()`      | path (dir)  |

Path parameters always use `type=str` (Python) or `type = "character"` (R)
because GeoEngine passes container paths as strings.

### 4.4 Numeric types: ALWAYS use `float` / `"double"` (Critical)

GeoEngine integrates with QGIS, which sends all numeric values as floats.
To prevent type-mismatch errors at runtime:

- **Python**: Always use `type=float` in `add_argument()`, even for
  parameters that look like integers (e.g., band indices, counts, pixel sizes).
- **R**: Always use `type = "double"` in `make_option()`, even for parameters
  with integer defaults (e.g., `4L`).

**Python -- WRONG:**
```python
parser.add_argument("--red-band", type=int, default=4)     # DO NOT USE
```

**Python -- CORRECT:**
```python
parser.add_argument("--red-band", type=float, default=4,
                    help="Band index for red reflectance (default: 4)")
```

If the function body needs an `int`, cast it after parsing:

```python
red_band = int(args.red_band)
```

**R -- CORRECT:**
```r
make_option("--red-band", type = "double", default = 4,
            dest = "red_band", help = "Band index for red [default: 4]")
```

If the function body needs an integer, cast after parsing:

```r
red_band <- as.integer(args$red_band)
```

---

## 5. Boolean Handling (Critical)

GeoEngine passes booleans as the **strings** `"true"` / `"false"`.

**Python -- WRONG:**
```python
parser.add_argument("--verbose", action="store_true")   # DO NOT USE
```

**Python -- CORRECT:**
```python
parser.add_argument("--verbose", type=str, default="false",
                    help="Enable verbose output")
# After parsing:
verbose = args.verbose.lower() in ("true", "1", "yes")
```

**R -- CORRECT:**
```r
make_option("--verbose", type = "character", default = "false",
            dest = "verbose", help = "Enable verbose output")
# After parsing:
verbose <- tolower(args$verbose) %in% c("true", "1", "yes")
```

---

## 6. Naming Convention

Function parameter names use **underscores** (snake_case). CLI flags use
**hyphens** (kebab-case). The conversion is:

```
Function param    CLI flag         Python attr       R dest
--------------    -----------      -----------       ------
input_dir     --> --input-dir  --> args.input_dir    args$input_dir
red_band      --> --red-band   --> args.red_band     args$red_band
output_dir    --> --output-dir --> args.output_dir   args$output_dir
```

- Replace underscores with hyphens when creating the `--flag` name.
- Python `argparse` automatically converts `--input-dir` back to `args.input_dir`.
- R `optparse` requires an explicit `dest = "input_dir"` parameter.

---

## 7. Python Template

Given this main function:

```python
def run_batch(input_dir, output_dir, to, recursive=False, overwrite=False,
              quality=92, compress="deflate", max_size=None):
    """Batch convert images in a folder."""
    ...
```

Generate this:

```python
import argparse


def parse_args():
    parser = argparse.ArgumentParser(
        description="Batch convert images in a folder."
    )
    # --- Required (no default in function signature) ---
    parser.add_argument(
        "--input-dir", type=str, required=True,
        help="Input directory"
    )
    parser.add_argument(
        "--output-dir", type=str, required=True,
        help="Output directory"
    )
    parser.add_argument(
        "--to", type=str, required=True,
        help="Target format"
    )
    # --- Optional (have defaults in function signature) ---
    parser.add_argument(
        "--recursive", type=str, default="false",
        help="Recurse into subfolders (default: false)"
    )
    parser.add_argument(
        "--overwrite", type=str, default="false",
        help="Overwrite existing outputs (default: false)"
    )
    parser.add_argument(
        "--quality", type=float, default=92,
        help="JPEG/WebP quality (default: 92)"
    )
    parser.add_argument(
        "--compress", type=str, default="deflate",
        help="GeoTIFF compression (default: deflate)"
    )
    parser.add_argument(
        "--max-size", type=float, default=None,
        help="Resize longest edge to this many px"
    )
    return parser.parse_args()


if __name__ == "__main__":
    args = parse_args()

    run_batch(
        input_dir  = args.input_dir,
        output_dir = args.output_dir,
        to         = args.to,
        recursive  = args.recursive.lower() in ("true", "1", "yes"),
        overwrite  = args.overwrite.lower() in ("true", "1", "yes"),
        quality    = args.quality,
        compress   = args.compress,
        max_size   = args.max_size,
    )
```

Note:

- `description` is taken from the function's docstring.
- `bool` params (`recursive`, `overwrite`) become `type=str` with string
  conversion (Section 5).
- Numeric params (`quality`, `max_size`) use `type=float` (Section 4.4).
- The `if __name__` block calls the original function with parsed args.

### Where to insert

- Define `parse_args()` **before** the `if __name__ == "__main__":` block.
- If the script has no `if __name__` block, create one.
- The `if __name__` block calls the identified main function with all parsed
  arguments mapped to their original parameter names.

---

## 8. R Template

Given this main function:

```r
calculate_ndvi <- function(input_file, output_dir, red_band = 4L,
                           nir_band = 5L, threshold = NULL) {
  #' Compute NDVI from a multispectral satellite image.
  ...
}
```

Generate this, appended **at the bottom of the script** after all function
definitions:

```r
# ---------------------------------------------------------------------------
# CLI argument parsing
# ---------------------------------------------------------------------------

suppressPackageStartupMessages(library(optparse))

option_list <- list(
  make_option(
    c("-f", "--input-file"),
    type    = "character",
    default = NULL,
    dest    = "input_file",
    help    = "Path to input file (required)"
  ),
  make_option(
    c("-o", "--output-dir"),
    type    = "character",
    default = NULL,
    dest    = "output_dir",
    help    = "Output directory for results (required)"
  ),
  make_option(
    c("-r", "--red-band"),
    type    = "double",
    default = 4,
    dest    = "red_band",
    help    = "Band index for red reflectance [default: 4]"
  ),
  make_option(
    c("-n", "--nir-band"),
    type    = "double",
    default = 5,
    dest    = "nir_band",
    help    = "Band index for NIR reflectance [default: 5]"
  ),
  make_option(
    c("-t", "--threshold"),
    type    = "double",
    default = NULL,
    dest    = "threshold",
    help    = "NDVI threshold for vegetation mask (optional)"
  )
)

parser <- OptionParser(
  usage       = "%prog [options]",
  description = "Compute NDVI from a multispectral satellite image.",
  option_list = option_list
)

args <- parse_args(parser)

# --- Validate required arguments ---
if (is.null(args$input_file)) {
  print_help(parser)
  stop("--input-file is required.", call. = FALSE)
}
if (is.null(args$output_dir)) {
  print_help(parser)
  stop("--output-dir is required.", call. = FALSE)
}

# --- Call main function ---
calculate_ndvi(
  input_file = normalizePath(args$input_file),
  output_dir = normalizePath(args$output_dir, mustWork = FALSE),
  red_band   = as.integer(args$red_band),
  nir_band   = as.integer(args$nir_band),
  threshold  = args$threshold
)
```

Note:

- `description` is taken from the function's roxygen/comment docstring.
- Numeric params use `type = "double"` (Section 4.4), then cast to integer
  with `as.integer()` when the function expects it.
- Required params (no default) use `default = NULL` + manual check.
- The final call maps every parsed arg to the original function parameter.

### Short flags for R

Assign single-letter short flags to the most common arguments:

| Short | Long flag      | Typical use       |
|-------|----------------|-------------------|
| `-i`  | `--input-dir`  | Input directory   |
| `-o`  | `--output-dir` | Output directory  |
| `-f`  | `--input-file` | Input file        |
| `-t`  | `--to`         | Target format     |
| `-r`  | `--red-band`   | Band index        |
| `-n`  | `--nir-band`   | Band index        |

Only assign short flags when there is no ambiguity. Skip them for less common
parameters.

---

## 9. Required Argument Validation

### Python

Use `required=True` directly in `add_argument()` for parameters that have
**no default** in the function signature:

```python
# Function: def process(input_dir, output_dir, threshold=0.5):
#                       ^^^^^^^^^^  ^^^^^^^^^^  <-- no default = required

parser.add_argument("--input-dir", type=str, required=True, help="...")
parser.add_argument("--output-dir", type=str, required=True, help="...")
parser.add_argument("--threshold", type=float, default=0.5, help="...")
```

### R

`optparse` has no `required` parameter. Instead, set `default = NULL` and
check after parsing:

```r
make_option("--input-dir", type = "character", default = NULL, dest = "input_dir")

# After parse_args():
if (is.null(args$input_dir)) {
  print_help(parser)
  stop("--input-dir is required.", call. = FALSE)
}
```

---

## 10. Procedure (Step-by-Step)

```
 START
   |
   v
 1. Find the script file (main.py / main.R, or scan directory)
   |
   v
 2. Check: does it already have an argument parser? (Section 1)
   |
   +-- YES --> STOP. Print: "Script already has an argument parser."
   |
   +-- NO
       |
       v
 3. Identify the main function (Section 2)
       |
       +-- UNCLEAR --> STOP. Ask the user which function to use.
       |
       +-- FOUND
           |
           v
 4. Extract the function's parameter list (Section 3)
       |
       v
 5. Determine language from file extension (.py or .R)
       |
       v
 6. For each parameter, determine:
       |   a. CLI flag name (underscore --> hyphen)
       |   b. argparse/optparse type (Section 4)
       |   c. Required or optional (Section 3.3)
       |   d. Default value (carry from signature)
       |
       v
 7. Generate the parser code (Section 7 for Python, Section 8 for R)
       |
       v
 8. Insert into the script:
       |   Python: parse_args() before __main__, call main function from __main__
       |   R:      optparse block at bottom, call main function at end
       |
       v
 9. Verify (Section 11)
       |
       v
 DONE
```

---

## 11. Verification Checklist

After generating, verify every item:

- [ ] Every function parameter has a matching `--flag` in the parser
- [ ] Every `--flag` in the parser maps to a function parameter
- [ ] Flag names use hyphens; `dest` / Python attr uses underscores
- [ ] Parameters without defaults are `required=True` (Python) or
      `default = NULL` + manual check (R)
- [ ] Parameters with defaults carry the same default value
- [ ] All numeric types use `type=float` (Python) / `type = "double"` (R)
- [ ] Boolean parameters are `type=str` with `"true"`/`"false"` conversion
- [ ] Path parameters are `type=str` / `type = "character"`
- [ ] The `if __name__` block (Python) or script tail (R) calls the
      original main function with all parsed arguments correctly mapped
- [ ] Integer casts (`int()` / `as.integer()`) are applied where the
      function body expects integer values
- [ ] No duplicate parser -- the script did not already have one
- [ ] `description` is taken from the function's docstring, not invented
