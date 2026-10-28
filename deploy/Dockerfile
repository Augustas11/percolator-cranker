# Keeper binaries for staging (public tree layout).
#
#   docker build -f deploy/Dockerfile -t percolator-keeper .
#   docker build -f deploy/Dockerfile --build-arg BINS="cranker staging_traders monitor" -t percolator-keeper .
#
# Builds the native (host-target) keeper binaries only. The SBF program build
# needs the Solana platform tools and stays out of the image: deploy the
# program with scripts/staging_devnet_deploy.sh. The image runs the binaries
# against the RPC and program id given at run time.
#
# BINS: `cranker` exists today. `staging_traders` and `monitor` come from
# separate changes; add them to BINS once they land.
#
# No secrets are baked in. Keypairs are mounted read-only at run time (see
# deploy/docker-compose.yml).

ARG RUST_IMAGE=rust:1.96-slim-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM ${RUST_IMAGE} AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates python3 \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# Wrapper checkout at the pinned SHA (path dependency of the crates).
RUN ./scripts/fetch_upstream.sh
ARG BINS="cranker"
RUN set -eu; args=""; for b in $BINS; do args="$args --bin $b"; done; \
    cargo build --release --locked $args; \
    mkdir -p /out; for b in $BINS; do cp "target/release/$b" /out/; done

FROM ${RUNTIME_IMAGE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10001 keeper \
 && useradd --system --uid 10001 --gid keeper --home-dir /keeper --create-home keeper
COPY --from=build /out/ /usr/local/bin/
# The keeper binaries read $HOME/.config/solana/id.json. Mount the role's
# keypair there read-only.
ENV HOME=/keeper
USER keeper
WORKDIR /keeper
ENTRYPOINT ["/usr/local/bin/cranker"]
