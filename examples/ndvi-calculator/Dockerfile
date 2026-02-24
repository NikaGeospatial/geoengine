FROM ghcr.io/prefix-dev/pixi:0.41.4 AS build

WORKDIR /pixi

COPY pixi.toml ./

RUN pixi install

RUN pixi shell-hook -s bash > /shell-hook

RUN echo '#!/bin/bash' > /pixi/entrypoint.sh && \
	cat /shell-hook >> /pixi/entrypoint.sh && \
	echo 'exec "$@"' >> /pixi/entrypoint.sh

FROM ubuntu:24.04 AS final

RUN apt-get update && apt-get install -y --no-install-recommends \
	ca-certificates \
	curl \
	git \
	&& rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/pixi /usr/local/bin/pixi
COPY --from=build /pixi/.pixi/envs/default /pixi/.pixi/envs/default
COPY --from=build --chmod=0755 /pixi/entrypoint.sh /pixi/entrypoint.sh
COPY --from=build /pixi/pixi.toml /pixi/pixi.toml

WORKDIR /app

COPY . /app

RUN useradd -m worker && chown -R worker:worker /app
USER worker

ENV PATH="/pixi/.pixi/envs/default/bin:${PATH}"
ENV GDAL_DATA="/pixi/.pixi/envs/default/share/gdal"
ENV PROJ_LIB="/pixi/.pixi/envs/default/share/proj"

ENV PYTHONUNBUFFERED="1"

ENTRYPOINT ["/pixi/entrypoint.sh"]