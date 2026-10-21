# The Linux release of Mini Remote, built by scripts/linux/build.sh.
#
# Ubuntu 22.04 has glibc 2.35, so the app runs on it and anything newer
# (Debian 12, Raspberry Pi OS 12 and so on). The window system (X11 or
# Wayland) and graphics drivers are loaded while the app runs, so nothing
# beyond glibc is linked in.

FROM ubuntu:22.04 AS toolchain
ARG RUST_VERSION
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      ca-certificates curl gcc g++ libc6-dev make nasm binutils \
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION"

# The 256x256 app icon (the same for every processor, so made natively).
FROM --platform=$BUILDPLATFORM ubuntu:22.04 AS icon
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends imagemagick \
 && rm -rf /var/lib/apt/lists/*
COPY assets/icon-1024.png /icon-1024.png
RUN convert /icon-1024.png -resize 256x256 -strip /mini-remote.png

FROM toolchain AS build
ARG VERSION
ARG ARCH
# Downloaded crates and build files are kept between builds (one build folder
# per processor), so rebuilding after a change is quick.
RUN --mount=type=bind,target=/src \
    --mount=type=cache,id=mini-remote-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=mini-remote-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=mini-remote-target-linux-$ARCH,target=/target \
    cd /src \
 && cargo build --release --locked -p mini-remote --target-dir /target \
 && install -D -m 755 /target/release/mini-remote /pkg/mini-remote/mini-remote \
 && strip /pkg/mini-remote/mini-remote \
 && echo "Needs $(objdump -T /pkg/mini-remote/mini-remote | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n 1) or newer"
COPY --from=icon /mini-remote.png /pkg/mini-remote/mini-remote.png
COPY LICENSE /pkg/mini-remote/LICENSE
COPY assets/fonts/Inter-LICENSE.txt /pkg/mini-remote/Inter-LICENSE.txt
COPY scripts/linux/mini-remote.desktop scripts/linux/install.sh /pkg/mini-remote/
RUN chmod 644 /pkg/mini-remote/*.png /pkg/mini-remote/*LICENSE* /pkg/mini-remote/mini-remote.desktop \
 && chmod 755 /pkg/mini-remote/install.sh \
 && mkdir /out \
 && tar -C /pkg --sort=name --owner=0 --group=0 --numeric-owner \
      -czf "/out/mini-remote-$VERSION-linux-$ARCH.tar.gz" mini-remote

FROM scratch
COPY --from=build /out/ /
