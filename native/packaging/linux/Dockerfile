# Entorno de compilación de los artefactos de Linux (AppImage y .deb).
#
# Un binario exige, como mínimo, la glibc de la máquina donde se compiló. Con
# Ubuntu 22.04 (glibc 2.35) los artefactos corren en Ubuntu 22.04+, Debian 12+,
# Fedora 36+, etc. Compilados directamente en el runner (Ubuntu 26.04, glibc
# 2.43) solo correrían en distribuciones de 2026 en adelante.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential ca-certificates clang cmake curl file git pkg-config python3 \
      dpkg-dev desktop-file-utils \
      libasound2-dev libfontconfig-dev libfreetype-dev libssl-dev libvulkan-dev \
      libwayland-dev libx11-xcb-dev libxcb1-dev libxkbcommon-dev \
      libxkbcommon-x11-dev libzstd-dev \
    && rm -rf /var/lib/apt/lists/*

# Toolchain de Rust en una ruta legible por cualquier usuario: el contenedor
# corre con el uid del anfitrión para no dejar archivos de root en el repo.
ARG RUST_TOOLCHAIN=stable
ENV RUSTUP_HOME=/opt/rustup \
    RUSTUP_TOOLCHAIN=$RUST_TOOLCHAIN \
    PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | CARGO_HOME=/opt/cargo sh -s -- -y --profile minimal --no-modify-path \
          --default-toolchain "$RUST_TOOLCHAIN" \
    && chmod -R a+rX /opt/rustup /opt/cargo
