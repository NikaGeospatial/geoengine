FROM nika-geoengine/geoengine-base:latest

COPY requirements.R /
RUN tr '\n' ' ' < requirements.R | \
	xargs pixi add && \
	pixi install -e r

COPY main.R /

ENTRYPOINT ["pixi", "run", "-e", "r"]