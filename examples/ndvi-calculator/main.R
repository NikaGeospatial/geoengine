#!/usr/bin/env Rscript
# NDVI Calculator — Computes the Normalized Difference Vegetation Index from
# multispectral satellite imagery (e.g. Landsat, Sentinel-2).
#
# NDVI = (NIR - Red) / (NIR + Red)
#
# The main entry point is `calculate_ndvi()`.

suppressPackageStartupMessages({
  library(optparse)
  library(terra)
})

# ---------------------------------------------------------------------------
# Core analysis
# ---------------------------------------------------------------------------

#' Calculate NDVI from a multispectral raster.
#'
#' This is the main function. It reads the input raster, extracts the red and
#' NIR bands, computes NDVI, and writes the result to disk.
#'
#' @param input_file  Character. Path to a multispectral GeoTIFF.
#' @param output_dir  Character. Directory where the NDVI raster is written.
#' @param red_band    Integer. Band index for red (default 4 for Landsat 8/9).
#' @param nir_band    Integer. Band index for NIR (default 5 for Landsat 8/9).
#' @param threshold   Numeric or NULL. If set, also produce a binary vegetation
#'                    mask where NDVI >= threshold.
#' @return Invisible NULL. Results are written to output_dir.
calculate_ndvi <- function(input_file, output_dir, red_band, nir_band,
                           threshold = NULL) {
  cat(sprintf("Reading input raster: %s\n", input_file))
  raster <- terra::rast(input_file)

  nbands <- terra::nlyr(raster)
  cat(sprintf("  Bands found: %d\n", nbands))

  if (red_band < 1 || nir_band < 1 || red_band > nbands || nir_band > nbands) {
    stop(sprintf(
      "Raster has %d bands but requested red=%d, nir=%d. %s",
      nbands, red_band, nir_band,
      "Check your --red-band and --nir-band values (must be >= 1 and <= number of bands)."
    ), call. = FALSE)
  }

  red <- raster[[red_band]]
  nir <- raster[[nir_band]]

  cat(sprintf("  Computing NDVI (NIR=band %d, Red=band %d)...\n",
              nir_band, red_band))

  # NDVI = (NIR - Red) / (NIR + Red), result in [-1, 1]
  ndvi <- (nir - red) / (nir + red)

  # Clamp to valid range
  ndvi <- terra::clamp(ndvi, lower = -1, upper = 1)

  dir.create(output_dir, recursive = TRUE, showWarnings = FALSE)

  # Write NDVI raster
  base_name <- tools::file_path_sans_ext(basename(input_file))
  ndvi_path <- file.path(output_dir, paste0(base_name, "_ndvi.tif"))
  terra::writeRaster(ndvi, ndvi_path, filetype = "GTiff", overwrite = TRUE)
  cat(sprintf("  NDVI raster written: %s\n", ndvi_path))

  # Print summary statistics
  vals <- terra::values(ndvi, na.rm = TRUE)
  cat(sprintf("  NDVI stats — min: %.4f  mean: %.4f  max: %.4f\n",
              min(vals), mean(vals), max(vals)))

  # Optional: produce a binary vegetation mask
  if (!is.null(threshold)) {
    mask <- ndvi >= threshold
    mask_path <- file.path(output_dir, paste0(base_name, "_veg_mask.tif"))
    terra::writeRaster(mask, mask_path, filetype = "GTiff",
                       datatype = "INT1U", overwrite = TRUE)
    veg_pct <- sum(terra::values(mask, na.rm = TRUE)) /
               length(terra::values(mask, na.rm = TRUE)) * 100
    cat(sprintf("  Vegetation mask written: %s  (%.1f%% above threshold %.2f)\n",
                mask_path, veg_pct, threshold))
  }

  cat("Done.\n")
  invisible(NULL)
}

# ---------------------------------------------------------------------------
# CLI argument parsing
# ---------------------------------------------------------------------------

option_list <- list(
  make_option(
    c("-i", "--input-file"),
    type    = "character",
    default = NULL,
    dest    = "input_file",
    help    = "Path to a multispectral GeoTIFF (required)"
  ),
  make_option(
    c("-o", "--output-dir"),
    type    = "character",
    default = NULL,
    dest    = "output_dir",
    help    = "Output directory for NDVI results (required)"
  ),
  make_option(
    c("-r", "--red-band"),
    type    = "double",
    default = 4,
    dest    = "red_band",
    help    = "Band index for red reflectance [default: 4 (Landsat 8/9)]"
  ),
  make_option(
    c("-n", "--nir-band"),
    type    = "double",
    default = 5,
    dest    = "nir_band",
    help    = "Band index for NIR reflectance [default: 5 (Landsat 8/9)]"
  ),
  make_option(
    c("-t", "--threshold"),
    type    = "double",
    default = NULL,
    dest    = "threshold",
    help    = "NDVI threshold for vegetation mask (optional, e.g. 0.3)"
  )
)

parser <- OptionParser(
  usage       = "%prog [options]",
  description = paste0(
    "Compute NDVI from a multispectral satellite image.\n",
    "Outputs a single-band GeoTIFF with values in [-1, 1].\n",
    "Optionally produces a binary vegetation mask at a given threshold."
  ),
  option_list = option_list
)

args <- parse_args(parser)

if (is.null(args$input_file)) {
  print_help(parser)
  stop("--input-file is required.", call. = FALSE)
}

if (is.null(args$output_dir)) {
  print_help(parser)
  stop("--output-dir is required.", call. = FALSE)
}

if (!file.exists(args$input_file)) {
  stop(sprintf("Input file does not exist: %s", args$input_file), call. = FALSE)
}

calculate_ndvi(
  input_file = normalizePath(args$input_file),
  output_dir = normalizePath(args$output_dir, mustWork = FALSE),
  red_band   = as.integer(args$red_band),
  nir_band   = as.integer(args$nir_band),
  threshold  = args$threshold
)
