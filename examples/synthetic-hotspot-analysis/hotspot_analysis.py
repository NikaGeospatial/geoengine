"""
Standalone geospatial hotspot analysis script (intentionally not a polished app).

This script generates synthetic incident data, builds a regular analysis grid,
computes multiple spatial features, and produces a composite hotspot score:

- severity-weighted incident density by grid cell
- inverse distance weighted (IDW) severity interpolation
- queen-neighbor spatial lag (local smoothing)
- distance-to-facility gap
- recency weighting
- contiguous hotspot cluster labeling

Dependencies (typical geospatial data-science stack):
    geopandas, shapely, pandas, numpy
"""

from __future__ import annotations

import argparse
from collections import deque
from dataclasses import dataclass
import json
import math
from pathlib import Path

import geopandas as gpd
import numpy as np
import pandas as pd
from shapely.geometry import Point, Polygon, box


@dataclass
class AnalysisInputs:
    study_area: gpd.GeoDataFrame
    incidents: gpd.GeoDataFrame
    facilities: gpd.GeoDataFrame
    neighborhoods: gpd.GeoDataFrame


def estimate_utm_epsg(lon: float, lat: float) -> int:
    zone = int((lon + 180.0) // 6.0) + 1
    return (32600 if lat >= 0 else 32700) + zone


def make_study_area() -> Polygon:
    coords = [
        (-122.55, 47.47),
        (-122.43, 47.45),
        (-122.31, 47.52),
        (-122.26, 47.63),
        (-122.33, 47.74),
        (-122.49, 47.78),
        (-122.60, 47.71),
        (-122.62, 47.57),
        (-122.55, 47.47),
    ]
    return Polygon(coords).buffer(0)


def random_points_in_polygon(
    polygon: Polygon,
    n: int,
    rng: np.random.Generator,
    cluster_centers: list[tuple[float, float]],
    cluster_spreads: list[tuple[float, float]],
    cluster_probs: list[float],
) -> np.ndarray:
    points: list[tuple[float, float]] = []
    probs = np.array(cluster_probs, dtype=float)
    probs = probs / probs.sum()

    while len(points) < n:
        center_idx = rng.choice(len(cluster_centers), p=probs)
        cx, cy = cluster_centers[center_idx]
        sx, sy = cluster_spreads[center_idx]
        x = rng.normal(cx, sx)
        y = rng.normal(cy, sy)
        if polygon.contains(Point(x, y)):
            points.append((x, y))

    return np.asarray(points)


def make_neighborhoods(study_area: Polygon, crs: str | object = "EPSG:4326") -> gpd.GeoDataFrame:
    minx, miny, maxx, maxy = study_area.bounds
    mx = (minx + maxx) / 2
    my = (miny + maxy) / 2

    raw = [
        ("Northwest", box(minx, my, mx, maxy)),
        ("Northeast", box(mx, my, maxx, maxy)),
        ("Southwest", box(minx, miny, mx, my)),
        ("Southeast", box(mx, miny, maxx, my)),
    ]
    rows = []
    for name, geom in raw:
        clipped = geom.intersection(study_area)
        if not clipped.is_empty:
            rows.append((name, clipped))

    return gpd.GeoDataFrame(rows, columns=["name", "geometry"], crs=crs)


def generate_synthetic_inputs(seed: int = 42) -> AnalysisInputs:
    rng = np.random.default_rng(seed)
    study_polygon = make_study_area()

    study_area = gpd.GeoDataFrame(
        {"name": ["study_area"]}, geometry=[study_polygon], crs="EPSG:4326"
    )

    incident_cluster_centers = [
        (-122.48, 47.56),
        (-122.39, 47.66),
        (-122.34, 47.57),
        (-122.51, 47.70),
    ]
    incident_cluster_spreads = [
        (0.018, 0.020),
        (0.015, 0.018),
        (0.013, 0.016),
        (0.014, 0.017),
    ]
    incident_cluster_probs = [0.30, 0.35, 0.22, 0.13]

    incident_xy = random_points_in_polygon(
        polygon=study_polygon,
        n=700,
        rng=rng,
        cluster_centers=incident_cluster_centers,
        cluster_spreads=incident_cluster_spreads,
        cluster_probs=incident_cluster_probs,
    )
    incident_cluster_idx = rng.choice(
        len(incident_cluster_centers), size=700, p=incident_cluster_probs
    )

    base_severity = rng.gamma(shape=2.0, scale=12.0, size=700)
    cluster_uplift = np.array([8.0, 18.0, 12.0, 6.0])[incident_cluster_idx]
    severity = np.clip(base_severity + cluster_uplift + rng.normal(0, 4, 700), 1, None)

    days_ago = rng.integers(0, 180, size=700)
    observed_at = pd.Timestamp("2026-02-01") - pd.to_timedelta(days_ago, unit="D")
    recency_weight = np.exp(-days_ago / 50.0)

    incidents = gpd.GeoDataFrame(
        {
            "incident_id": np.arange(1, 701),
            "cluster_seed": incident_cluster_idx,
            "severity": severity,
            "days_ago": days_ago,
            "recency_weight": recency_weight,
            "observed_at": observed_at,
        },
        geometry=gpd.points_from_xy(incident_xy[:, 0], incident_xy[:, 1]),
        crs="EPSG:4326",
    )

    facility_xy = random_points_in_polygon(
        polygon=study_polygon,
        n=18,
        rng=rng,
        cluster_centers=[
            (-122.50, 47.60),
            (-122.41, 47.63),
            (-122.36, 47.70),
            (-122.44, 47.53),
        ],
        cluster_spreads=[(0.030, 0.025)] * 4,
        cluster_probs=[0.2, 0.35, 0.2, 0.25],
    )
    facilities = gpd.GeoDataFrame(
        {"facility_id": np.arange(1, 19)},
        geometry=gpd.points_from_xy(facility_xy[:, 0], facility_xy[:, 1]),
        crs="EPSG:4326",
    )

    neighborhoods = make_neighborhoods(study_polygon, crs="EPSG:4326")

    return AnalysisInputs(
        study_area=study_area,
        incidents=incidents,
        facilities=facilities,
        neighborhoods=neighborhoods,
    )


def _ensure_crs(gdf: gpd.GeoDataFrame, label: str) -> gpd.GeoDataFrame:
    if gdf.crs is None:
        raise ValueError(f"{label} layer has no CRS. Set a CRS before running analysis.")
    return gdf


def _single_study_area_polygon(study_area: gpd.GeoDataFrame) -> gpd.GeoDataFrame:
    union_geom = study_area.geometry.unary_union
    if union_geom.is_empty:
        raise ValueError("Study area geometry is empty after reading file.")
    return gpd.GeoDataFrame({"name": ["study_area"]}, geometry=[union_geom], crs=study_area.crs)


def _coerce_point_geometry(gdf: gpd.GeoDataFrame, label: str) -> gpd.GeoDataFrame:
    geom_types = set(gdf.geometry.geom_type.dropna().astype(str))
    pointish = {"Point", "MultiPoint"}
    if geom_types and geom_types.issubset(pointish):
        return gdf

    out = gdf.copy()
    out.geometry = out.geometry.centroid
    return out


def _clip_to_study_area(gdf: gpd.GeoDataFrame, study_area: gpd.GeoDataFrame) -> gpd.GeoDataFrame:
    if len(gdf) == 0:
        return gdf.copy()
    study_geom = study_area.geometry.iloc[0]
    mask = gdf.geometry.intersects(study_geom)
    clipped = gdf.loc[mask].copy()
    return clipped.reset_index(drop=True)


def _prepare_incidents_for_analysis(
    incidents: gpd.GeoDataFrame,
    severity_col: str,
    observed_at_col: str | None,
    analysis_date: str | pd.Timestamp | None,
    recency_halflife_days: float,
) -> gpd.GeoDataFrame:
    out = incidents.copy()

    if severity_col in out.columns:
        out["severity"] = pd.to_numeric(out[severity_col], errors="coerce").fillna(1.0)
    else:
        out["severity"] = 1.0
    out["severity"] = out["severity"].clip(lower=0.0)

    out["incident_id"] = np.arange(1, len(out) + 1, dtype=int)

    if observed_at_col and observed_at_col in out.columns:
        observed = pd.to_datetime(out[observed_at_col], errors="coerce")
    else:
        observed = pd.Series(pd.NaT, index=out.index, dtype="datetime64[ns]")

    if observed.notna().any():
        if analysis_date is None:
            anchor = observed.max().normalize()
        else:
            anchor = pd.Timestamp(analysis_date).normalize()
        days_ago = (anchor - observed).dt.days
        days_ago = days_ago.fillna(days_ago.median() if days_ago.notna().any() else 0)
        days_ago = days_ago.clip(lower=0)
        out["observed_at"] = observed
        out["days_ago"] = days_ago.astype(int)
        decay_scale = recency_halflife_days / math.log(2.0)
        out["recency_weight"] = np.exp(-out["days_ago"] / decay_scale)
    else:
        out["observed_at"] = pd.NaT
        out["days_ago"] = 0
        out["recency_weight"] = 1.0

    return out


def _prepare_facilities_for_analysis(facilities: gpd.GeoDataFrame) -> gpd.GeoDataFrame:
    out = facilities.copy()
    out["facility_id"] = np.arange(1, len(out) + 1, dtype=int)
    return out


def _prepare_neighborhoods_for_analysis(
    neighborhoods: gpd.GeoDataFrame | None,
    study_area: gpd.GeoDataFrame,
    neighborhood_name_col: str,
) -> gpd.GeoDataFrame:
    if neighborhoods is None:
        return make_neighborhoods(study_area.geometry.iloc[0], crs=study_area.crs)

    out = neighborhoods.copy()
    if neighborhood_name_col in out.columns:
        out["name"] = out[neighborhood_name_col].astype(str)
    else:
        out["name"] = [f"neighborhood_{i}" for i in range(1, len(out) + 1)]

    clipped_geom = out.geometry.intersection(study_area.geometry.iloc[0])
    out = out.assign(geometry=clipped_geom)
    out = out.loc[~out.geometry.is_empty].copy()
    return out[["name", "geometry"]].reset_index(drop=True)


def load_inputs_from_files(
    study_area_path: str | Path,
    incidents_path: str | Path,
    facilities_path: str | Path,
    neighborhoods_path: str | Path | None = None,
    *,
    severity_col: str = "severity",
    observed_at_col: str | None = "observed_at",
    neighborhood_name_col: str = "name",
    analysis_date: str | pd.Timestamp | None = None,
    recency_halflife_days: float = 45.0,
) -> AnalysisInputs:
    study_area_raw = _ensure_crs(gpd.read_file(study_area_path), "study_area")
    incidents_raw = _ensure_crs(gpd.read_file(incidents_path), "incidents")
    facilities_raw = _ensure_crs(gpd.read_file(facilities_path), "facilities")
    neighborhoods_raw = None
    if neighborhoods_path is not None:
        neighborhoods_raw = _ensure_crs(gpd.read_file(neighborhoods_path), "neighborhoods")

    study_area = _single_study_area_polygon(study_area_raw)
    incidents = incidents_raw.to_crs(study_area.crs)
    facilities = facilities_raw.to_crs(study_area.crs)
    neighborhoods = neighborhoods_raw.to_crs(study_area.crs) if neighborhoods_raw is not None else None

    incidents = _coerce_point_geometry(incidents, "incidents")
    facilities = _coerce_point_geometry(facilities, "facilities")

    incidents = _clip_to_study_area(incidents, study_area)
    facilities = _clip_to_study_area(facilities, study_area)
    if neighborhoods is not None:
        neighborhoods = _clip_to_study_area(neighborhoods, study_area)

    incidents = _prepare_incidents_for_analysis(
        incidents=incidents,
        severity_col=severity_col,
        observed_at_col=observed_at_col,
        analysis_date=analysis_date,
        recency_halflife_days=recency_halflife_days,
    )
    facilities = _prepare_facilities_for_analysis(facilities)
    neighborhoods = _prepare_neighborhoods_for_analysis(
        neighborhoods=neighborhoods,
        study_area=study_area,
        neighborhood_name_col=neighborhood_name_col,
    )

    if len(facilities) == 0:
        raise ValueError("No facilities remain inside the study area after clipping.")

    return AnalysisInputs(
        study_area=study_area,
        incidents=incidents,
        facilities=facilities,
        neighborhoods=neighborhoods,
    )


def build_square_grid(study_area_projected: gpd.GeoDataFrame, cell_size_m: float) -> gpd.GeoDataFrame:
    geom = study_area_projected.geometry.iloc[0]
    minx, miny, maxx, maxy = geom.bounds
    xs = np.arange(minx, maxx, cell_size_m)
    ys = np.arange(miny, maxy, cell_size_m)

    cells = []
    for row_idx, y in enumerate(ys):
        for col_idx, x in enumerate(xs):
            square = box(x, y, x + cell_size_m, y + cell_size_m)
            if not square.intersects(geom):
                continue
            clipped = square.intersection(geom)
            if clipped.is_empty:
                continue
            cells.append((row_idx, col_idx, clipped))

    grid = gpd.GeoDataFrame(cells, columns=["row", "col", "geometry"], crs=study_area_projected.crs)
    grid["cell_area_m2"] = grid.geometry.area
    grid["cell_area_km2"] = grid["cell_area_m2"] / 1_000_000.0
    return grid.reset_index(drop=True)


def compute_cell_incident_stats(grid: gpd.GeoDataFrame, incidents: gpd.GeoDataFrame) -> gpd.GeoDataFrame:
    joined = gpd.sjoin(
        incidents,
        grid[["row", "col", "geometry"]],
        how="left",
        predicate="within",
    )

    joined["severity_x_recency"] = joined["severity"] * joined["recency_weight"]

    grouped = joined.groupby("index_right", dropna=True).agg(
        incident_count=("incident_id", "size"),
        severity_mean=("severity", "mean"),
        severity_sum=("severity", "sum"),
        weighted_burden=("severity_x_recency", "sum"),
        recency_mean=("recency_weight", "mean"),
        newest_event=("observed_at", "max"),
    )

    result = grid.join(grouped, how="left")
    fill_zero_cols = [
        "incident_count",
        "severity_mean",
        "severity_sum",
        "weighted_burden",
        "recency_mean",
    ]
    for col in fill_zero_cols:
        result[col] = result[col].fillna(0.0)
    result["incident_count"] = result["incident_count"].astype(int)
    result["burden_density"] = np.where(
        result["cell_area_km2"] > 0,
        result["weighted_burden"] / result["cell_area_km2"],
        0.0,
    )
    return result


def idw_interpolate_to_cells(
    grid: gpd.GeoDataFrame,
    points: gpd.GeoDataFrame,
    value_col: str,
    power: float = 2.0,
    search_radius_m: float = 3500.0,
    max_neighbors: int = 25,
) -> np.ndarray:
    centroids = grid.geometry.centroid
    gx = centroids.x.to_numpy()
    gy = centroids.y.to_numpy()
    px = points.geometry.x.to_numpy()
    py = points.geometry.y.to_numpy()
    values = points[value_col].to_numpy(dtype=float)

    if len(points) == 0:
        return np.zeros(len(grid), dtype=float)

    dx = gx[:, None] - px[None, :]
    dy = gy[:, None] - py[None, :]
    dist = np.sqrt(dx * dx + dy * dy)

    neighbor_count = min(max_neighbors, dist.shape[1])
    nearest_idx = np.argpartition(dist, kth=neighbor_count - 1, axis=1)[:, :neighbor_count]
    nearest_dist = np.take_along_axis(dist, nearest_idx, axis=1)
    nearest_vals = values[nearest_idx]

    within_radius = nearest_dist <= search_radius_m
    safe_dist = np.maximum(nearest_dist, 1.0)
    weights = np.where(within_radius, 1.0 / np.power(safe_dist, power), 0.0)

    exact_hit = nearest_dist < 1.0
    exact_any = exact_hit.any(axis=1)
    out = np.zeros(len(grid), dtype=float)

    if exact_any.any():
        exact_weighted = np.where(exact_hit, nearest_vals, np.nan)
        out[exact_any] = np.nanmean(exact_weighted[exact_any], axis=1)

    remaining = ~exact_any
    if remaining.any():
        w = weights[remaining]
        v = nearest_vals[remaining]
        denom = w.sum(axis=1)
        out[remaining] = np.where(
            denom > 0,
            (w * v).sum(axis=1) / denom,
            0.0,
        )
    return out


def nearest_point_distance_to_cells(grid: gpd.GeoDataFrame, facilities: gpd.GeoDataFrame) -> np.ndarray:
    centroids = grid.geometry.centroid
    gx = centroids.x.to_numpy()
    gy = centroids.y.to_numpy()
    fx = facilities.geometry.x.to_numpy()
    fy = facilities.geometry.y.to_numpy()

    dx = gx[:, None] - fx[None, :]
    dy = gy[:, None] - fy[None, :]
    dist = np.sqrt(dx * dx + dy * dy)
    return dist.min(axis=1)


def queen_spatial_lag(grid: gpd.GeoDataFrame, value_col: str) -> np.ndarray:
    values = grid[value_col].to_numpy(dtype=float)
    coord_to_idx = {(int(r), int(c)): i for i, (r, c) in enumerate(zip(grid["row"], grid["col"]))}
    lag = np.zeros(len(grid), dtype=float)

    offsets = [
        (-1, -1), (-1, 0), (-1, 1),
        (0, -1),           (0, 1),
        (1, -1),  (1, 0),  (1, 1),
    ]

    for i, (r, c) in enumerate(zip(grid["row"].astype(int), grid["col"].astype(int))):
        neighbors = []
        for dr, dc in offsets:
            j = coord_to_idx.get((r + dr, c + dc))
            if j is not None:
                neighbors.append(values[j])
        lag[i] = float(np.mean(neighbors)) if neighbors else 0.0

    return lag


def robust_minmax(series: pd.Series, invert: bool = False) -> pd.Series:
    x = pd.Series(series, copy=True).astype(float)
    if x.isna().all():
        out = pd.Series(np.zeros(len(x)), index=x.index, dtype=float)
        return 1.0 - out if invert else out

    x = x.fillna(x.median())
    lo, hi = x.quantile([0.05, 0.95])
    if not np.isfinite(lo) or not np.isfinite(hi) or math.isclose(lo, hi):
        out = pd.Series(np.zeros(len(x)), index=x.index, dtype=float)
        return 1.0 - out if invert else out

    x = x.clip(lower=lo, upper=hi)
    out = (x - lo) / (hi - lo)
    return 1.0 - out if invert else out


def label_hotspot_clusters(grid: gpd.GeoDataFrame, hotspot_col: str = "is_hotspot") -> gpd.GeoDataFrame:
    coord_to_idx = {(int(r), int(c)): i for i, (r, c) in enumerate(zip(grid["row"], grid["col"]))}
    hotspot_coords = {
        (int(r), int(c))
        for r, c, hot in zip(grid["row"], grid["col"], grid[hotspot_col])
        if bool(hot)
    }

    cluster_id = np.zeros(len(grid), dtype=int)
    cluster_size = np.zeros(len(grid), dtype=int)
    visited: set[tuple[int, int]] = set()
    next_cluster = 1

    offsets = [
        (-1, -1), (-1, 0), (-1, 1),
        (0, -1),           (0, 1),
        (1, -1),  (1, 0),  (1, 1),
    ]

    for start in list(hotspot_coords):
        if start in visited:
            continue

        q: deque[tuple[int, int]] = deque([start])
        members: list[int] = []
        visited.add(start)

        while q:
            r, c = q.popleft()
            idx = coord_to_idx[(r, c)]
            members.append(idx)
            for dr, dc in offsets:
                nxt = (r + dr, c + dc)
                if nxt in hotspot_coords and nxt not in visited:
                    visited.add(nxt)
                    q.append(nxt)

        for idx in members:
            cluster_id[idx] = next_cluster
            cluster_size[idx] = len(members)
        next_cluster += 1

    out = grid.copy()
    out["hotspot_cluster_id"] = cluster_id
    out["hotspot_cluster_size"] = cluster_size
    return out


def summarize_by_neighborhood(
    grid: gpd.GeoDataFrame,
    neighborhoods: gpd.GeoDataFrame,
) -> pd.DataFrame:
    centroids = grid.copy()
    centroids.geometry = centroids.geometry.centroid
    joined = gpd.sjoin(
        centroids,
        neighborhoods[["name", "geometry"]],
        how="left",
        predicate="within",
    )

    summary = (
        joined.groupby("name", dropna=False)
        .agg(
            cells=("row", "size"),
            incidents=("incident_count", "sum"),
            mean_score=("hotspot_score", "mean"),
            hotspot_cells=("is_hotspot", "sum"),
            hotspot_clusters=("hotspot_cluster_id", lambda s: int((s > 0).sum())),
            avg_facility_distance_m=("facility_distance_m", "mean"),
        )
        .sort_values(["mean_score", "incidents"], ascending=[False, False])
        .reset_index()
    )
    return summary


def _run_hotspot_analysis_core(
    inputs: AnalysisInputs,
    *,
    cell_size_m: float = 1200.0,
    idw_power: float = 1.8,
    idw_search_radius_m: float = 4500.0,
    idw_max_neighbors: int = 30,
    min_incidents_per_active_cell: int = 3,
    hotspot_quantile: float = 0.90,
) -> dict[str, object]:
    lon, lat = inputs.study_area.geometry.iloc[0].centroid.x, inputs.study_area.geometry.iloc[0].centroid.y
    projected_epsg = estimate_utm_epsg(lon, lat)
    projected_crs = f"EPSG:{projected_epsg}"

    study_area_p = inputs.study_area.to_crs(projected_crs)
    incidents_p = inputs.incidents.to_crs(projected_crs)
    facilities_p = inputs.facilities.to_crs(projected_crs)
    neighborhoods_p = inputs.neighborhoods.to_crs(projected_crs)

    if len(facilities_p) == 0:
        raise ValueError("Facilities layer is empty. At least one facility point is required.")

    grid = build_square_grid(study_area_projected=study_area_p, cell_size_m=cell_size_m)
    grid = compute_cell_incident_stats(grid=grid, incidents=incidents_p)

    grid["idw_severity"] = idw_interpolate_to_cells(
        grid=grid,
        points=incidents_p,
        value_col="severity",
        power=idw_power,
        search_radius_m=idw_search_radius_m,
        max_neighbors=idw_max_neighbors,
    )
    grid["facility_distance_m"] = nearest_point_distance_to_cells(grid, facilities_p)
    grid["spatial_lag_burden"] = queen_spatial_lag(grid, "burden_density")

    grid["n_burden_density"] = robust_minmax(grid["burden_density"])
    grid["n_idw_severity"] = robust_minmax(grid["idw_severity"])
    grid["n_spatial_lag"] = robust_minmax(grid["spatial_lag_burden"])
    grid["n_facility_gap"] = robust_minmax(grid["facility_distance_m"])
    grid["n_recency"] = robust_minmax(grid["recency_mean"])

    grid["hotspot_score"] = (
        0.34 * grid["n_burden_density"]
        + 0.24 * grid["n_idw_severity"]
        + 0.20 * grid["n_spatial_lag"]
        + 0.12 * grid["n_facility_gap"]
        + 0.10 * grid["n_recency"]
    )

    active_cells = grid["incident_count"] >= min_incidents_per_active_cell
    threshold = (
        grid.loc[active_cells, "hotspot_score"].quantile(hotspot_quantile)
        if active_cells.any()
        else 1.0
    )
    grid["is_hotspot"] = (grid["hotspot_score"] >= threshold) & active_cells

    grid = label_hotspot_clusters(grid, hotspot_col="is_hotspot")

    neighborhood_summary = summarize_by_neighborhood(grid, neighborhoods_p)

    hotspot_cells = (
        grid.loc[grid["is_hotspot"]]
        .sort_values(["hotspot_score", "incident_count"], ascending=[False, False])
        .copy()
    )

    return {
        "projected_crs": projected_crs,
        "study_area": study_area_p,
        "incidents": incidents_p,
        "facilities": facilities_p,
        "neighborhoods": neighborhoods_p,
        "grid": grid,
        "hotspot_cells": hotspot_cells,
        "neighborhood_summary": neighborhood_summary,
    }


def run_hotspot_analysis(seed: int = 42) -> dict[str, object]:
    inputs = generate_synthetic_inputs(seed=seed)
    return _run_hotspot_analysis_core(inputs)


def run_hotspot_analysis_from_files(
    study_area_path: str | Path,
    incidents_path: str | Path,
    facilities_path: str | Path,
    neighborhoods_path: str | Path | None = None,
    *,
    severity_col: str = "severity",
    observed_at_col: str | None = "observed_at",
    neighborhood_name_col: str = "name",
    analysis_date: str | pd.Timestamp | None = None,
    recency_halflife_days: float = 45.0,
    cell_size_m: float = 1200.0,
    idw_power: float = 1.8,
    idw_search_radius_m: float = 4500.0,
    idw_max_neighbors: int = 30,
    min_incidents_per_active_cell: int = 3,
    hotspot_quantile: float = 0.90,
) -> dict[str, object]:
    """
    Run hotspot analysis from real geospatial files.

    Expected inputs:
    - study_area_path: polygon layer (one or more polygons)
    - incidents_path: point layer (or non-point, coerced to centroids)
      Optional columns:
        * severity_col (numeric): incident severity/weight; defaults to 1 if missing
        * observed_at_col (datetime-like): used for recency weighting; flat weight if missing
    - facilities_path: point layer (or non-point, coerced to centroids)
    - neighborhoods_path: optional polygon layer for summary reporting

    Returns the same dictionary as `run_hotspot_analysis(...)`.
    """
    inputs = load_inputs_from_files(
        study_area_path=study_area_path,
        incidents_path=incidents_path,
        facilities_path=facilities_path,
        neighborhoods_path=neighborhoods_path,
        severity_col=severity_col,
        observed_at_col=observed_at_col,
        neighborhood_name_col=neighborhood_name_col,
        analysis_date=analysis_date,
        recency_halflife_days=recency_halflife_days,
    )
    return _run_hotspot_analysis_core(
        inputs,
        cell_size_m=cell_size_m,
        idw_power=idw_power,
        idw_search_radius_m=idw_search_radius_m,
        idw_max_neighbors=idw_max_neighbors,
        min_incidents_per_active_cell=min_incidents_per_active_cell,
        hotspot_quantile=hotspot_quantile,
    )


def write_hotspot_analysis_outputs(
    results: dict[str, object],
    output_folder: str | Path,
) -> dict[str, Path]:
    output_dir = Path(output_folder)
    output_dir.mkdir(parents=True, exist_ok=True)

    grid = results["grid"]
    hotspot_cells = results["hotspot_cells"]
    neighborhood_summary = results["neighborhood_summary"]

    if not isinstance(grid, gpd.GeoDataFrame):
        raise TypeError("Expected 'grid' result to be a GeoDataFrame.")
    if not isinstance(hotspot_cells, gpd.GeoDataFrame):
        raise TypeError("Expected 'hotspot_cells' result to be a GeoDataFrame.")
    if not isinstance(neighborhood_summary, pd.DataFrame):
        raise TypeError("Expected 'neighborhood_summary' result to be a DataFrame.")

    grid_path = output_dir / "grid.gpkg"
    hotspot_cells_path = output_dir / "hotspot_cells.gpkg"
    neighborhood_summary_path = output_dir / "neighborhood_summary.csv"
    run_summary_path = output_dir / "run_summary.json"

    grid.to_file(grid_path, layer="grid", driver="GPKG")
    hotspot_cells.to_file(hotspot_cells_path, layer="hotspot_cells", driver="GPKG")
    neighborhood_summary.to_csv(neighborhood_summary_path, index=False)

    hotspot_cluster_count = int(
        grid.loc[grid["hotspot_cluster_id"] > 0, "hotspot_cluster_id"].nunique()
    )
    run_summary = {
        "projected_crs": results["projected_crs"],
        "grid_cells": int(len(grid)),
        "incidents": int(len(results["incidents"])),
        "facilities": int(len(results["facilities"])),
        "hotspot_cells": int(grid["is_hotspot"].sum()),
        "hotspot_clusters": hotspot_cluster_count,
        "outputs": {
            "grid_gpkg": str(grid_path),
            "hotspot_cells_gpkg": str(hotspot_cells_path),
            "neighborhood_summary_csv": str(neighborhood_summary_path),
        },
    }
    run_summary_path.write_text(json.dumps(run_summary, indent=2), encoding="utf-8")

    return {
        "grid": grid_path,
        "hotspot_cells": hotspot_cells_path,
        "neighborhood_summary": neighborhood_summary_path,
        "run_summary": run_summary_path,
    }


def main(
    study_area_path: str,
    incidents_path: str,
    facilities_path: str,
    output_folder: str,
    neighborhoods_path: str | None = None,
    severity_col: str = "severity",
    observed_at_col: str | None = "observed_at",
    neighborhood_name_col: str = "name",
    analysis_date: str | None = None,
    recency_halflife_days: float = 45.0,
    cell_size_m: float = 1200.0,
    idw_power: float = 1.8,
    idw_search_radius_m: float = 4500.0,
    idw_max_neighbors: int = 30,
    min_incidents_per_active_cell: int = 3,
    hotspot_quantile: float = 0.90,
) -> dict[str, Path]:
    results = run_hotspot_analysis_from_files(
        study_area_path=study_area_path,
        incidents_path=incidents_path,
        facilities_path=facilities_path,
        neighborhoods_path=neighborhoods_path,
        severity_col=severity_col,
        observed_at_col=observed_at_col,
        neighborhood_name_col=neighborhood_name_col,
        analysis_date=analysis_date,
        recency_halflife_days=recency_halflife_days,
        cell_size_m=cell_size_m,
        idw_power=idw_power,
        idw_search_radius_m=idw_search_radius_m,
        idw_max_neighbors=idw_max_neighbors,
        min_incidents_per_active_cell=min_incidents_per_active_cell,
        hotspot_quantile=hotspot_quantile,
    )
    return write_hotspot_analysis_outputs(results, output_folder=output_folder)


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run hotspot analysis from geospatial input files and write outputs."
    )
    parser.add_argument(
        "--study-area-path",
        type=str,
        required=True,
        help="Path to a polygon study area layer.",
    )
    parser.add_argument(
        "--incidents-path",
        type=str,
        required=True,
        help="Path to an incidents layer (points or geometries coerced to centroids).",
    )
    parser.add_argument(
        "--facilities-path",
        type=str,
        required=True,
        help="Path to a facilities layer (points or geometries coerced to centroids).",
    )
    parser.add_argument(
        "--output-folder",
        type=str,
        required=True,
        help="Writable folder where analysis outputs will be written.",
    )
    parser.add_argument(
        "--neighborhoods-path",
        type=str,
        default=None,
        help="Optional polygon layer for neighborhood summaries.",
    )
    parser.add_argument(
        "--severity-col",
        type=str,
        default="severity",
        help="Incident severity column name (default: severity).",
    )
    parser.add_argument(
        "--observed-at-col",
        type=str,
        default="observed_at",
        help="Incident datetime column for recency weighting (default: observed_at).",
    )
    parser.add_argument(
        "--neighborhood-name-col",
        type=str,
        default="name",
        help="Neighborhood name column (default: name).",
    )
    parser.add_argument(
        "--analysis-date",
        type=str,
        required=False,
        help="Optional analysis anchor date/time (ISO-like string).",
    )
    parser.add_argument(
        "--recency-halflife-days",
        type=float,
        default=45.0,
        help="Recency weighting half-life in days (default: 45.0).",
    )
    parser.add_argument(
        "--cell-size-m",
        type=float,
        default=1200.0,
        help="Grid cell size in meters (default: 1200.0).",
    )
    parser.add_argument(
        "--idw-power",
        type=float,
        default=1.8,
        help="IDW interpolation power (default: 1.8).",
    )
    parser.add_argument(
        "--idw-search-radius-m",
        type=float,
        default=4500.0,
        help="IDW search radius in meters (default: 4500.0).",
    )
    parser.add_argument(
        "--idw-max-neighbors",
        type=float,
        default=30,
        help="Maximum IDW neighbors (default: 30).",
    )
    parser.add_argument(
        "--min-incidents-per-active-cell",
        type=float,
        default=3,
        help="Minimum incidents for a cell to be considered active (default: 3).",
    )
    parser.add_argument(
        "--hotspot-quantile",
        type=float,
        default=0.90,
        help="Quantile threshold among active cells for hotspot labeling (default: 0.90).",
    )
    return parser


def _normalize_optional_string(value: str | None) -> str | None:
    if value is None:
        return None
    stripped = value.strip()
    if stripped == "" or stripped.lower() in {"none", "null"}:
        return None
    return stripped


def _cli_main() -> dict[str, Path]:
    parser = _build_arg_parser()
    args = parser.parse_args()

    outputs = main(
        study_area_path=args.study_area_path,
        incidents_path=args.incidents_path,
        facilities_path=args.facilities_path,
        output_folder=args.output_folder,
        neighborhoods_path=_normalize_optional_string(args.neighborhoods_path),
        severity_col=args.severity_col,
        observed_at_col=_normalize_optional_string(args.observed_at_col),
        neighborhood_name_col=args.neighborhood_name_col,
        analysis_date=_normalize_optional_string(args.analysis_date),
        recency_halflife_days=float(args.recency_halflife_days),
        cell_size_m=float(args.cell_size_m),
        idw_power=float(args.idw_power),
        idw_search_radius_m=float(args.idw_search_radius_m),
        idw_max_neighbors=int(args.idw_max_neighbors),
        min_incidents_per_active_cell=int(args.min_incidents_per_active_cell),
        hotspot_quantile=float(args.hotspot_quantile),
    )

    print("Hotspot analysis complete.")
    for key, path in outputs.items():
        print(f"{key}: {path}")
    return outputs


if __name__ == "__main__":
    _cli_main()
