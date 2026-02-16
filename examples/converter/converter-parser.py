#!/usr/bin/env python3
import argparse
import importlib.util
import json
import os
import sys
import uuid
from pathlib import Path

PY_FILE = (Path(__file__).resolve().parent / "main.py").resolve()
FUNCTION = "convert_one_direct"

def load_function(py_file, function_name):
    module_name = "geoengine_user_module"
    spec = importlib.util.spec_from_file_location(module_name, py_file)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Unable to load module spec from {py_file}")
    mod = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = mod
    spec.loader.exec_module(mod)
    return getattr(mod, function_name)

def parse_bool(value):
    v = str(value).strip().lower()
    if v in ("1", "true", "yes", "y", "on"):
        return True
    if v in ("0", "false", "no", "n", "off"):
        return False
    raise argparse.ArgumentTypeError(f"Invalid bool: {value}")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--src', dest='src', type=str, required=True, help="Auto-generated for 'src'")
    parser.add_argument('--to', dest='to', type=str, required=True, help="Auto-generated for 'to'")
    parser.add_argument('--overwrite', dest='overwrite', type=parse_bool, default=True, help="Auto-generated for 'overwrite'")
    parser.add_argument('--compress', dest='compress', type=str, default="deflate", help="Auto-generated for 'compress'")
    parser.add_argument('--max-size', dest='max_size', type=int, help="Auto-generated for 'max_size'")
    parser.add_argument('--quality', dest='quality', type=int, default=92, help="Auto-generated for 'quality'")
    args = parser.parse_args()
    output_dir = Path(os.environ.get("GEOENGINE_OUTPUT_DIR", "tool-parsers")).resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    output_target = str(output_dir / f"{uuid.uuid4().hex}.txt")

    fn = load_function(PY_FILE, FUNCTION)
    result = fn(src=args.src, dst=output_target, to=args.to, overwrite=args.overwrite, compress=args.compress, max_size=args.max_size, quality=args.quality)

    try:
        print(json.dumps(result))
    except TypeError:
        print(result)

if __name__ == "__main__":
    main()
