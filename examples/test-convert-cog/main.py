"""Convert a GeoTIFF to a Cloud-Optimized GeoTIFF (COG) using GDAL."""

import argparse
import subprocess
import sys
from pathlib import Path


def parse_args():
    parser = argparse.ArgumentParser(
        description="Translate a GeoTIFF to COG format using gdal.Translate."
    )
    parser.add_argument(
        "--input-path", type=str, required=True,
        help="Path to the input GeoTIFF file"
    )
    parser.add_argument(
        "--output-path", type=str, required=True,
        help="Path for the output COG file"
    )
    return parser.parse_args()


def convert_to_cog(input_path: str, output_path: str) -> None:
    """Translate a GeoTIFF to COG format using gdal_translate."""
    cmd = [
        "gdal_translate",
        "-of", "COG",
        "-co", "COMPRESS=DEFLATE",
        "-co", "OVERVIEW_RESAMPLING=NEAREST",
        input_path,
        output_path,
    ]

    result = subprocess.run(cmd, capture_output=True, text=True)

    if result.returncode != 0:
        print(f"Error: gdal_translate failed for '{input_path}'")
        print(result.stderr)
        sys.exit(1)

    print(f"COG written to {output_path}")


if __name__ == "__main__":
    args = parse_args()

    convert_to_cog(
        input_path  = args.input_path,
        output_path = args.output_path,
    )
