---
name: use-geoengine
description: >
  Master routing skill for GeoEngine CLI workflows. Use this when a user asks
  anything about GeoEngine: creating a new worker from scratch, updating an
  existing worker after script changes, editing a main script and syncing the
  worker, running or inspecting workers, managing images, or deploying to the
  cloud. This skill routes to the correct sub-skills and CLI commands and
  explains every GeoEngine command with its flags.
---

# Use GeoEngine

GeoEngine is a Docker-based isolated runtime manager for geospatial workloads.
It packages a user's script (Python or R) into a reproducible Docker image and
exposes it to GIS plugins (QGIS, ArcGIS) or direct CLI invocation.

---

## Workflow Routing

Read the user's intent and route to the correct workflow below.

| User intent | Workflow to follow |
|---|---|
| Create a new worker from scratch | [Workflow 1 – New Worker](#workflow-1--new-worker-from-scratch) |
| Updated their script and want worker artifacts refreshed | [Workflow 2 – Update Worker After Script Changes](#workflow-2--update-worker-after-script-changes) |
| Want to change something in the main script | [Workflow 3 – Edit Script Then Optionally Update Worker](#workflow-3--edit-script-then-optionally-update-worker) |
| Run, inspect, diff, delete, manage images, or deploy | [CLI Reference](#cli-reference) |
| Upgraded GeoEngine and want to patch all artifacts and plugins | [`geoengine patch`](#geoengine-patch) |

---

## Workflow 1 – New Worker from Scratch

**Trigger:** The user wants to generate a new GeoEngine worker project.

Delegate entirely to the **`make-geoengine-worker`** skill. That skill handles
`geoengine init`, `write-argparse`, `write-geoengine-yaml`, `write-pixi-toml`,
`geoengine apply`, and `geoengine build --dev` in the correct order.

---

## Workflow 2 – Update Worker After Script Changes

**Trigger:** The user has already modified the main script (`main.py` /
`main.R`) and wants the worker artifacts and Docker image updated to match.

Run these steps in order:

1. Use the **`write-pixi-toml`** skill to align `pixi.toml` dependencies with
   any new or removed imports in the updated script.
2. Use the **`write-geoengine-yaml`** skill to sync `geoengine.yaml` inputs and
   command contract with any changed or added argument-parser flags.
3. Run `geoengine apply` from inside the worker directory to register the
   updated configuration and regenerate the Dockerfile.
4. Run `geoengine build --dev` to rebuild the development Docker image.

> If a step fails, surface the exact error and fix the local/configuration
> cause before retrying. Do not skip steps.

---

## Workflow 3 – Edit Script Then Optionally Update Worker

**Trigger:** The user asks you to change, fix, or refactor code in the main
script.

1. Make the requested change to the script.
2. After the edit is complete, **ask the user**: "Would you like me to update
   the worker artifacts and rebuild the Docker image for this change?"
3. If yes, follow [Workflow 2](#workflow-2--update-worker-after-script-changes).
4. If no, stop — the user will rebuild manually when ready.

---

## CLI Reference

All commands support `-v` / `--verbose` for extra output and `-h` / `--help`
for usage.

### `geoengine init`

Initialize a new worker directory. Creates `geoengine.yaml` and starter files.

```
geoengine init [OPTIONS]
```

| Flag | Description |
|---|---|
| `-n, --name <NAME>` | Worker name (kebab-case). Defaults to the current directory name. |
| `-e, --env <ENV>` | Language environment: `py` (default) or `r`. |

**When to use:** Only at the very start of a new worker project, before any
other commands. Usually called by the `make-geoengine-worker` skill.

---

### `geoengine apply`

Register a new worker or update an existing one. Reads `geoengine.yaml`,
validates it, installs/updates GIS plugin entries, and regenerates the
Dockerfile.

```
geoengine apply [WORKER]
```

| Argument | Description |
|---|---|
| `[WORKER]` | Worker name or path to worker directory. Defaults to the current directory. |

**When to use:** Every time `geoengine.yaml` changes (new inputs, version bump,
plugin toggle, mount changes). Must be run before `geoengine build` for changes
to take effect.

---

### `geoengine build`

Build the Docker image for a worker.

```
geoengine build [OPTIONS]
```

| Flag | Description |
|---|---|
| `--dev` | Build a development image (relaxed versioning). Use during active development. |
| `--no-cache` | Force a clean rebuild; ignore Docker layer cache. |
| `--build-arg <KEY=VALUE>` | Pass custom Docker build arguments. Repeatable. |

**When to use:**
- `geoengine build --dev` — fast iteration during development; always run after
  `geoengine apply`.
- `geoengine build` — production build once the worker is stable.

---

### `geoengine run`

Run a worker with input parameters. Translates `KEY=VALUE` pairs into
`--key value` CLI flags passed to the script inside Docker.

```
geoengine run [OPTIONS] [WORKER] [-- <ARGS>...]
```

| Flag / Argument | Description |
|---|---|
| `[WORKER]` | Worker name or path. Defaults to current directory. |
| `-i, --input <KEY=VALUE>` | Input parameter. Repeatable. Maps to `--key value` inside the container. |
| `--dev` | Run the dev image (built with `--dev`). |
| `--json` | Emit structured JSON result to stdout; logs go to stderr. |
| `[-- <ARGS>...]` | Extra raw arguments passed through to the container command. |

**Example:**
```bash
geoengine run -i input-file=/data/raster.tif -i output-dir=/output --dev
```

> If a `file` parameter in `geoengine.yaml` declares `filetypes`, `geoengine run`
> validates the file extension early and bails with a clear error if it does not
> match. For input files (`readonly: true`) `filetypes` lists the accepted input
> formats; for output files (`readonly: false`) it lists the formats the script
> produces. Omit `filetypes` (or set it to `[".*"]`) for no restriction.

---

### `geoengine workers`

List all registered workers.

```
geoengine workers [OPTIONS]
```

| Flag | Description |
|---|---|
| `--json` | Output as JSON for programmatic use. |
| `--gis <GIS>` | Filter to workers registered in a specific GIS plugin: `qgis` or `arcgis`. |

---

### `geoengine describe`

Describe a specific worker: shows name, version, inputs, plugins, and mounts.

```
geoengine describe [WORKER]
```

| Argument / Flag | Description |
|---|---|
| `[WORKER]` | Worker name or path. Defaults to current directory. |
| `--json` | Output as JSON. |

---

### `geoengine diff`

Check for differences between the current worker files and what GeoEngine has
on record (useful to see what has changed since the last `apply`).

```
geoengine diff [OPTIONS]
```

| Flag | Description |
|---|---|
| `-f, --file <FILE>` | Scope the diff: `all` (default), `config` (geoengine.yaml only), `dockerfile` (Dockerfile only), or `worker` (worker directory). |

---

### `geoengine delete`

Delete a worker from GeoEngine (unregisters it; does not delete source files).

```
geoengine delete [OPTIONS]
```

| Flag | Description |
|---|---|
| `-n, --name <NAME>` | Worker name to delete. Defaults to current directory's worker. |

---

### `geoengine image`

Manage Docker images under GeoEngine.

```
geoengine image <SUBCOMMAND>
```

| Subcommand | Description |
|---|---|
| `list` | List all GeoEngine Docker images. |
| `import` | Import a Docker image from a `.tar` file (for air-gapped environments). |
| `remove` | Remove a GeoEngine Docker image. |

---

### `geoengine deploy`

Deploy images to GCP Artifact Registry.

```
geoengine deploy <SUBCOMMAND>
```

| Subcommand | Description |
|---|---|
| `auth` | Authenticate with GCP Artifact Registry. |
| `push` | Push a local image to the registry. |
| `pull` | Pull an image from the registry. |
| `list` | List images available in the registry. |

---

### `geoengine patch`

Validate all GeoEngine-managed artifacts and repair anything that is stale. Run
this after upgrading GeoEngine to bring all workers and GIS plugins up to date
in one shot.

```
geoengine patch
```

No flags. The command:

1. **Global artifacts** — parses `~/.geoengine/settings.yaml`, every
   `state/*.yaml`, and every `configs/*.json`; reports parse errors and
   orphaned files (files with no matching registered worker).
2. **Per-worker** — for every registered worker: checks the path exists,
   validates `geoengine.yaml` schema (read-only), checks `pixi.toml` is
   present (read-only), and silently regenerates `Dockerfile` and
   `.dockerignore` if their content differs from the current canonical
   template.
3. **GIS plugins** — hashes each installed QGIS and ArcGIS plugin file
   against the canonical version embedded in the binary. Reinstalls
   automatically if stale; skips entirely if the GIS application is not
   installed on the machine.

Exits non-zero if any validation error is found (parse failures, missing
paths, reinstall failures).

**When to use:** After upgrading GeoEngine. Not needed as part of the normal
`init → apply → build` development loop.

---

## Key Artifacts

| File | Purpose                                                                 | Updated by |
|---|-------------------------------------------------------------------------|---|
| `geoengine.yaml` | Worker identity, command, inputs, mounts, plugins                       | `write-geoengine-yaml` skill |
| `pixi.toml` | Conda + PyPI dependency environment                                     | `write-pixi-toml` skill |
| `Dockerfile` | Container build definition (auto-generated) **Do NOT ever touch this.** | `geoengine apply` / `geoengine patch` |
| `.dockerignore` | Files to exclude from Docker build context (auto-generated) **Do NOT ever touch this.** | `geoengine apply` / `geoengine patch` |

---

## Typical Command Sequences

**New worker (full setup):**
```bash
geoengine init                 # creates geoengine.yaml and starter files
# ... write-argparse, write-geoengine-yaml, write-pixi-toml skills run here
geoengine apply                # validates config, generates Dockerfile
geoengine build --dev          # builds dev Docker image
```

**After updating a script:**
```bash
# ... write-pixi-toml and write-geoengine-yaml skills run here
geoengine apply
geoengine build --dev
```

**Inspect and test:**
```bash
geoengine describe             # view worker config
geoengine workers              # list all workers
geoengine diff                 # see what has changed since last apply
geoengine run -i key=value --dev   # execute the worker locally
```

**Production build and deploy:**
```bash
geoengine build                # production image (no --dev)
geoengine deploy auth          # authenticate with GCP
geoengine deploy push          # push image to registry
```

**After upgrading GeoEngine:**
```bash
geoengine patch                # validate all artifacts, patch stale Dockerfiles and GIS plugins
```
