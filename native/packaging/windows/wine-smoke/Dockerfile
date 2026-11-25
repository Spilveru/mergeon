# Imagen para probar el instalador NSIS bajo wine (ver run.sh).
# El instalador de NSIS es de 32 bits: hace falta wine32.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN dpkg --add-architecture i386 \
 && apt-get update \
 && apt-get install -y --no-install-recommends wine wine32:i386 wine64 nsis xvfb xauth ca-certificates \
 && rm -rf /var/lib/apt/lists/*
