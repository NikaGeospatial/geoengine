# GeoEngine

A Docker-based isolated runtime manager for geospatial workloads — with GPU acceleration, GIS (ArcGIS Pro and QGIS) integration and interoperability, and AI agent support.

## Features

- **Isolated Execution**: Run Python/R scripts in Docker containers with GDAL, PyTorch, and other geospatial libraries
- **GPU-Ready**: NVIDIA GPU passthrough for CUDA-accelerated processing
- **GIS Integration**: Native plugins for ArcGIS Pro and QGIS -- tools run directly via the CLI, no proxy service required
- **Worker Management**: Declarative YAML configuration — `apply` registers, builds, and tags Docker images in one step
- **AI-Enabled**: Agent skills to automate the entire workflow
