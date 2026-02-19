"""Batch image conversion helpers with optional GeoTIFF-aware processing."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Optional, Tuple

import numpy as np
from PIL import Image, UnidentifiedImageError

import rasterio
from rasterio.enums import Resampling


SUPPORTED_EXTS = {".jpg", ".jpeg", ".png", ".bmp", ".webp", ".tif", ".tiff"}
TIFF_EXTS = {".tif", ".tiff"}

# Map target key -> (file extension, pillow format OR rasterio driver)
TARGETS = {
    "jpg": (".jpg", "JPEG"),
    "jpeg": (".jpeg", "JPEG"),
    "png": (".png", "PNG"),
    "bmp": (".bmp", "BMP"),
    "webp": (".webp", "WEBP"),
    "tif": (".tif", "TIFF"),        # written via rasterio path in code
    "tiff": (".tiff", "TIFF"),      # written via rasterio path in code
    "geotiff": (".tif", "GTiff"),   # written via rasterio
    "gtiff": (".tif", "GTiff"),     # alias
}


@dataclass(frozen=True)
class Options:
    """Runtime options for batch and direct image conversion operations."""
    input_dir: Path
    output_dir: Path
    to: str
    recursive: bool
    overwrite: bool
    keep_structure: bool
    quality: int
    compress: str
    max_size: Optional[int]


def iter_images(root: Path, recursive: bool) -> Iterable[Path]:
    """Yield supported image files from a directory tree."""
    if recursive:
        for p in root.rglob("*"):
            if p.is_file() and p.suffix.lower() in SUPPORTED_EXTS:
                yield p
    else:
        for p in root.iterdir():
            if p.is_file() and p.suffix.lower() in SUPPORTED_EXTS:
                yield p


def ensure_parent(path: Path) -> None:
    """Create the destination parent directory when it does not exist."""
    path.parent.mkdir(parents=True, exist_ok=True)


def resize_pil(img: Image.Image, max_size: Optional[int]) -> Image.Image:
    """Resize an image so its longest edge is at most `max_size`."""
    if not max_size:
        return img
    w, h = img.size
    longest = max(w, h)
    if longest <= max_size:
        return img
    scale = max_size / float(longest)
    new_w = max(1, int(round(w * scale)))
    new_h = max(1, int(round(h * scale)))
    return img.resize((new_w, new_h), resample=Image.Resampling.LANCZOS)


def rasterio_to_pil(ds: rasterio.DatasetReader) -> Image.Image:
    """Convert a rasterio dataset to a Pillow image."""
    data = ds.read()  # (count, H, W)
    count, h, w = data.shape

    # Normalize to uint8 if needed
    if data.dtype != np.uint8:
        out = np.zeros((count, h, w), dtype=np.uint8)
        for i in range(count):
            band = data[i].astype(np.float32)
            mn = np.nanmin(band)
            mx = np.nanmax(band)
            if not np.isfinite(mn) or not np.isfinite(mx) or mx <= mn:
                out[i] = 0
            else:
                out[i] = np.clip((band - mn) * 255.0 / (mx - mn), 0, 255).astype(np.uint8)
        data = out

    if count == 1:
        return Image.fromarray(data[0], mode="L")

    # Use first 3/4 bands
    if count >= 4:
        rgba = np.transpose(data[:4], (1, 2, 0))  # HxWx4
        return Image.fromarray(rgba, mode="RGBA")
    rgb = np.transpose(data[:3], (1, 2, 0))  # HxWx3
    return Image.fromarray(rgb, mode="RGB")


def pil_to_rasterio_arr(img: Image.Image) -> np.ndarray:
    """Convert a Pillow image to a CxHxW uint8 array for rasterio writes."""
    img = img.copy()
    if img.mode not in ("L", "RGB", "RGBA"):
        img = img.convert("RGB")
    arr = np.array(img)
    if arr.ndim == 2:  # HxW
        arr = arr[np.newaxis, :, :]  # 1xHxW
    else:  # HxWxC -> CxHxW
        arr = np.transpose(arr, (2, 0, 1))
    return arr.astype(np.uint8, copy=False)


def save_pil(img: Image.Image, out_path: Path, to_key: str, quality: int) -> None:
    """Save an image through Pillow using output settings for the target format."""
    ext, fmt = TARGETS[to_key]

    # JPEG can't handle alpha
    if fmt == "JPEG" and img.mode in ("RGBA", "LA"):
        img = img.convert("RGB")

    kwargs = {}
    if fmt in ("JPEG", "WEBP"):
        kwargs["quality"] = int(quality)
        kwargs["optimize"] = True

    ensure_parent(out_path)
    img.save(out_path, format=fmt, **kwargs)


def save_geotiff_from_raster(
    src_path: Path,
    out_path: Path,
    driver: str,
    compress: str,
    max_size: Optional[int],
) -> None:
    """
    TIFF/GeoTIFF -> (Geo)TIFF preserving transform/CRS/tags.
    """
    with rasterio.open(src_path) as src:
        profile = src.profile.copy()
        profile.update(driver=driver)

        # compression only applies to GTiff
        if driver == "GTiff":
            profile.update(compress=compress)

        if max_size:
            h, w = src.height, src.width
            longest = max(w, h)
            if longest > max_size:
                scale = max_size / float(longest)
                new_w = max(1, int(round(w * scale)))
                new_h = max(1, int(round(h * scale)))
                data = src.read(
                    out_shape=(src.count, new_h, new_w),
                    resampling=Resampling.bilinear,
                )
                transform = src.transform * src.transform.scale((w / new_w), (h / new_h))
                profile.update(height=new_h, width=new_w, transform=transform)
            else:
                data = src.read()
        else:
            data = src.read()

        ensure_parent(out_path)
        with rasterio.open(out_path, "w", **profile) as dst:
            dst.write(data)
            # copy tags (best-effort)
            try:
                dst.update_tags(**src.tags())
                for i in range(1, src.count + 1):
                    dst.update_tags(i, **src.tags(i))
            except Exception:
                pass


def save_geotiff_from_pil(
    img: Image.Image,
    out_path: Path,
    driver: str,
    compress: str,
    max_size: Optional[int],
) -> None:
    """
    Non-TIFF source -> (Geo)TIFF (no georeferencing unless you add it).
    """
    img = resize_pil(img, max_size)
    arr = pil_to_rasterio_arr(img)
    count, height, width = arr.shape

    profile = {
        "driver": "GTiff" if driver == "GTiff" else "GTiff",
        "height": height,
        "width": width,
        "count": count,
        "dtype": "uint8",
    }
    if compress and profile["driver"] == "GTiff":
        profile["compress"] = compress

    ensure_parent(out_path)
    with rasterio.open(out_path, "w", **profile) as dst:
        dst.write(arr)


def _convert_image(
    src_path: Path,
    dst_path: Path,
    to_key: str,
    compress: str,
    max_size: Optional[int],
    quality: int,
) -> Tuple[bool, str]:
    """Convert one image path to the requested output format."""
    out_ext, out_fmt = TARGETS[to_key]
    src_ext = src_path.suffix.lower()

    try:
        # If target is TIFF/GeoTIFF -> write via rasterio to support geotags if possible
        if out_ext in (".tif", ".tiff") or out_fmt == "GTiff":
            # GTiff is used for both tif/tiff and geotiff output paths.
            driver = "GTiff"
            if src_ext in TIFF_EXTS:
                save_geotiff_from_raster(
                    src_path,
                    dst_path,
                    driver=driver,
                    compress=compress,
                    max_size=max_size,
                )
            else:
                with Image.open(src_path) as im:
                    save_geotiff_from_pil(
                        im,
                        dst_path,
                        driver=driver,
                        compress=compress,
                        max_size=max_size,
                    )
            return True, "OK"

        # If source is TIFF/GeoTIFF but target is non-TIFF -> read via rasterio then write via Pillow
        if src_ext in TIFF_EXTS:
            with rasterio.open(src_path) as ds:
                im = rasterio_to_pil(ds)
            im = resize_pil(im, max_size)
            save_pil(im, dst_path, to_key, quality)
            return True, "OK"

        # Everything else -> Pillow roundtrip
        with Image.open(src_path) as im:
            im = resize_pil(im, max_size)
            save_pil(im, dst_path, to_key, quality)
        return True, "OK"

    except UnidentifiedImageError:
        return False, "Failed (unrecognized image)"
    except Exception as e:
        return False, f"Failed ({type(e).__name__}: {e})"


def convert_one(src: Path, dst: Path, opts: Options) -> Tuple[bool, str]:
    """Convert one file using the `Options` container."""
    if dst.exists() and not opts.overwrite:
        return False, "Skipped (exists)"

    return _convert_image(
        src_path=src,
        dst_path=dst,
        to_key=opts.to.lower(),
        compress=opts.compress,
        max_size=opts.max_size,
        quality=opts.quality,
    )

def convert_one_direct(src: Path,
                       dst: Path,
                       to: str,
                       overwrite: bool = True,
                       compress: str = "deflate",
                       max_size: int = None,
                       quality: int = 92) -> Tuple[bool, str]:
    """Convert one file using direct keyword arguments."""
    src_path = Path(src)
    dst_path = Path(dst)
    if dst_path.exists() and not overwrite:
        return False, "Skipped (exists)"

    return _convert_image(
        src_path=src_path,
        dst_path=dst_path,
        to_key=to.lower(),
        compress=compress,
        max_size=max_size,
        quality=quality,
    )


def run_batch(opts: Options) -> int:
    """Convert all supported files in the configured input directory."""
    opts.output_dir.mkdir(parents=True, exist_ok=True)
    any_fail = False

    for src in iter_images(opts.input_dir, opts.recursive):
        rel = src.relative_to(opts.input_dir)
        out_suffix = TARGETS[opts.to.lower()][0]
        out_rel = rel.with_suffix(out_suffix)

        dst = (opts.output_dir / out_rel) if opts.keep_structure else (opts.output_dir / out_rel.name)

        ok, msg = convert_one(src, dst, opts)
        print(f"{src} -> {dst} :: {msg}")
        if not ok and not msg.startswith("Skipped"):
            any_fail = True

    return 1 if any_fail else 0


def parse_args() -> Options:
    """Build command-line arguments and return a validated `Options` object."""
    parser = argparse.ArgumentParser(
        prog="imgconv",
        description="Batch convert images in a folder (JPEG/PNG/BMP/WebP/TIFF/GeoTIFF).",
    )
    parser.add_argument("--input-dir", "-i", default="data", help="Input directory (default: data)")
    parser.add_argument("--output-dir", "-o", default="output", help="Output directory (default: output)")
    parser.add_argument("--to", "-t", required=True, help=f"Target: {', '.join(sorted(TARGETS.keys()))}")
    parser.add_argument("--recursive", "-r", action="store_true", help="Recurse into subfolders")
    parser.add_argument("--overwrite", action="store_true", help="Overwrite existing outputs")
    parser.add_argument("--no-keep-structure", action="store_true", help="Flatten output (don’t mirror folders)")
    parser.add_argument("--quality", type=int, default=92, help="JPEG/WebP quality (default: 92)")
    parser.add_argument(
        "--compress",
        default="deflate",
        help="GeoTIFF compression (default: deflate). Common: deflate, lzw, zstd, none",
    )
    parser.add_argument("--max-size", type=int, default=None, help="Resize longest edge to this many px")

    args = parser.parse_args()
    print(args)
    to_key = args.to.lower()
    if to_key not in TARGETS:
        parser.error(f"--to must be one of: {', '.join(sorted(TARGETS.keys()))}")

    return Options(
        input_dir=Path(args.input_dir).resolve(),
        output_dir=Path(args.output_dir).resolve(),
        to=to_key,
        recursive=args.recursive,
        overwrite=args.overwrite,
        keep_structure=not args.no_keep_structure,
        quality=args.quality,
        compress=args.compress if args.compress != "none" else "",
        max_size=args.max_size,
    )


def main() -> None:
    """Run the CLI entrypoint and exit with the batch status code."""
    opts = parse_args()
    raise SystemExit(run_batch(opts))


if __name__ == "__main__":
    main()
