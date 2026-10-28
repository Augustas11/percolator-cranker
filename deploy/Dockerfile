# Day-0 bots + monitor image (docs/LAUNCH.md, "Hosting").
# Reference / dry-run only; not a paid live service; M3 NO-GO.
# Build from the repo root with the pinned wrapper checked out
# (scripts/fetch_upstream.sh): docker compose -f deploy/docker-compose.yml build
# The root .dockerignore keeps launch-runs/ (keys) and targets out of the context.
FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --manifest-path staging/Cargo.toml \
 && cargo build --release --manifest-path monitor/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/staging/target/release/staging /src/monitor/target/release/monitor /usr/local/bin/
COPY launch/sol-perp.toml /etc/percolator/sol-perp.toml
# Compose sets the user to the run dir's owner (LAUNCH_UID/LAUNCH_GID).
USER 10001
