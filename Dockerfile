# syntax=docker/dockerfile:1
# SnapRequest server: `docker build -t snapreq .` then
# `docker run -p 127.0.0.1:7878:7878 -v snapreq-data:/data snapreq`.

FROM docker.io/library/node:24-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY web/ ./
RUN npm run build

FROM docker.io/library/rust:1-bookworm AS build
WORKDIR /src
COPY . .
# The web UI is embedded into the binary (ADR-021).
COPY --from=web /web/dist web/dist
# Use the image's stable toolchain instead of installing the components pinned for development.
RUN rm -f rust-toolchain.toml
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p snapreq-cli \
    && cp target/release/snapreq /snapreq
# The data volume belongs to the unprivileged user of the runtime image.
RUN mkdir -p /data/workspace && chown -R 65532:65532 /data

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /snapreq /usr/local/bin/snapreq
COPY --from=build --chown=65532:65532 /data /data
# Inside the container the server must listen on all interfaces; publish the port on the
# host's loopback (`-p 127.0.0.1:7878:7878`) unless the network is trusted. With
# SNAPREQ_ALLOW_REMOTE the outbound policy blocks cloud metadata endpoints by default.
ENV SNAPREQ_BIND=0.0.0.0:7878 \
    SNAPREQ_ALLOW_REMOTE=1 \
    SNAPREQ_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 7878
USER nonroot:nonroot
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s \
    CMD ["/usr/local/bin/snapreq", "send", "http://127.0.0.1:7878/healthz", "--fail", "-t", "3"]
ENTRYPOINT ["/usr/local/bin/snapreq"]
CMD ["serve"]
