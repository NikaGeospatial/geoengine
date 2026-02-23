---
name: make-geoengine-worker
description: End-to-end workflow for creating a GeoEngine worker in a user-specified directory using the GeoEngine CLI plus the `write-argparse`, `write-geoengine-yaml`, and `write-pixi-toml` skills. Handles `geoengine init`, `apply`, and `build --dev`, including accepting plugin installation prompts.
---

# Make GeoEngine Worker

Use this skill when the user wants you to create or finish a GeoEngine worker
project in a specific directory.

## Scope Rules

- Run the entire workflow inside the directory the user specifies.
- If no directory is specified, ask for it before proceeding.
- Do not run commands from another directory and do not spread files across
  multiple folders unless the user asks.

## Required Flow (Do In This Order)

1. Change into the user-specified directory.
2. Run GeoEngine initialization:
   - Python project: `geoengine init`
   - R project: `geoengine init -e r`
3. Use `write-argparse` skill.
4. Use `write-geoengine-yaml` skill.
5. Use `write-pixi-toml` skill.
6. Run `geoengine apply` to save/validate `geoengine.yaml` and generate the
   Dockerfile.
   - If prompted to install any plugin, choose `yes`.
7. Run `geoengine build --dev` to create a development build without strict
   versioning requirements.
8. Report completion and give the user the follow-up commands/reminders from
   the "Completion Message" section below.

## Execution Notes

- Determine whether the project is Python or R from the user request and/or the
  project files (`main.py` vs `main.R`, etc.).
- Use the `-e r` flag only for R projects.
- If `geoengine init` created starter files that conflict with existing user
  files, inspect and merge carefully; do not overwrite user work blindly.
- If `geoengine apply` or `geoengine build --dev` fails, surface the exact error
  and fix what is local/configuration-related before stopping.

## Skill Chaining Guidance

This is an orchestration skill. After initialization, explicitly invoke:

- `write-argparse` to ensure the script accepts CLI flags GeoEngine passes
- `write-geoengine-yaml` to define worker inputs/command/plugins/mounts
- `write-pixi-toml` to define the runtime environment

Run them in that order because downstream files depend on earlier outputs.

## Completion Message (Required)

When the workflow succeeds, tell the user it is done and include:

- Run `geoengine describe` inside the worker directory to view worker details.
- Run `geoengine workers` to list registered workers.
- If a GIS plugin (e.g., QGIS/ArcGIS plugin) was enabled/installed, restart the
  GIS software before using the worker.
- Any time `geoengine.yaml` changes, run:
  1. `geoengine apply`
  2. `geoengine build` (or `geoengine build --dev` while developing)
