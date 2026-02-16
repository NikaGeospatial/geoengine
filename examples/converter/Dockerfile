FROM ghcr.io/osgeo/gdal:ubuntu-small-latest

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 python3-rasterio python3-numpy python3-pil \
    && rm -rf /var/lib/apt/lists/*

ENV PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1

COPY converter-parser.py /converter-parser.py
COPY main.py /main.py

