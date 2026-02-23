"""
Generate synthetic hotspot-analysis input layers and write them to disk.

This is a helper script for quickly producing sample inputs that can be passed
to the GeoEngine worker in this directory.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from hotspot_analysis import generate_synthetic_inputs


def write_sample_data(output_folder: str | Path, seed: int = 42) -> dict[str, Path]:
    out_dir = Path(output_folder)
    out_dir.mkdir(parents=True, exist_ok=True)

    inputs = generate_synthetic_inputs(seed=seed)

    study_area_path = out_dir / "study_area.gpkg"
    incidents_path = out_dir / "incidents.gpkg"
    facilities_path = out_dir / "facilities.gpkg"
    neighborhoods_path = out_dir / "neighborhoods.gpkg"
    manifest_path = out_dir / "sample_data_manifest.json"

    for path in [study_area_path, incidents_path, facilities_path, neighborhoods_path]:
        if path.exists():
            path.unlink()

    inputs.study_area.to_file(study_area_path, layer="study_area", driver="GPKG")
    inputs.incidents.to_file(incidents_path, layer="incidents", driver="GPKG")
    inputs.facilities.to_file(facilities_path, layer="facilities", driver="GPKG")
    inputs.neighborhoods.to_file(neighborhoods_path, layer="neighborhoods", driver="GPKG")

    manifest = {
        "seed": seed,
        "files": {
            "study_area": str(study_area_path),
            "incidents": str(incidents_path),
            "facilities": str(facilities_path),
            "neighborhoods": str(neighborhoods_path),
        },
        "worker_example": {
            "study-area-path": str(study_area_path),
            "incidents-path": str(incidents_path),
            "facilities-path": str(facilities_path),
            "neighborhoods-path": str(neighborhoods_path),
            "output-folder": str(out_dir / "worker_output"),
        },
    }
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    return {
        "study_area": study_area_path,
        "incidents": incidents_path,
        "facilities": facilities_path,
        "neighborhoods": neighborhoods_path,
        "manifest": manifest_path,
    }


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Generate synthetic hotspot analysis input layers."
    )
    parser.add_argument(
        "--output-folder",
        type=str,
        required=True,
        help="Folder where sample data files will be written.",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed used to generate synthetic data (default: 42).",
    )
    return parser


def main() -> None:
    args = _build_arg_parser().parse_args()
    outputs = write_sample_data(output_folder=args.output_folder, seed=int(args.seed))

    print("Sample data written:")
    for name, path in outputs.items():
        print(f"{name}: {path}")


if __name__ == "__main__":
    main()
