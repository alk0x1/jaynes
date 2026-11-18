# THROWAWAY. A Linux with a browser, a virtual screen and a pointer, to run the spike with no person and no display.
FROM node:24-bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends chromium xvfb xdotool imagemagick fonts-dejavu-core ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /spike
