# THROWAWAY (reach spike). A Linux with a browser, a firewall and bwrap, for the proxy, the pretend web and the boxes.
FROM node:24-bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends chromium iptables util-linux bubblewrap socat fonts-dejavu-core ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /spike
