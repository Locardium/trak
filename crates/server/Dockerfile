# Trak server in a container (Phase 5.6). Built from the repo root, since the
# server shares the workspace with trak-core:
#
#   docker build -f crates/server/Dockerfile -t trak-server .
#
# Everything the server keeps (config.toml, data/, music/) lives in the
# /srv/trak volume, so the container can be replaced without losing anything.
# See crates/server/README.md, "Docker".

FROM rust:1.96-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p trak-server \
    && cp target/release/trak-server /trak-server

FROM alpine:3.22
RUN adduser -D -H -u 10001 trak \
    && mkdir -p /srv/trak \
    && chown trak:trak /srv/trak
COPY --from=build /trak-server /usr/local/bin/trak-server
USER trak
# The defaults in config.toml (data_dir = "data", music_dir = "music") are
# relative to where the server runs: here, inside the volume.
WORKDIR /srv/trak
VOLUME /srv/trak
EXPOSE 7420
ENTRYPOINT ["trak-server"]
