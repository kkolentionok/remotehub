# ---- build stage ----
FROM rust:1-bookworm AS build
WORKDIR /app

# Copy manifest first for layer caching, then sources.
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

# ---- runtime stage ----
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Non-root runtime user; /data is the persisted SQLite volume.
RUN useradd --system --uid 10001 app && mkdir -p /data && chown app /data
USER app

COPY --from=build /app/target/release/rh-sync-server /usr/local/bin/rh-sync-server

ENV BIND_ADDR=0.0.0.0:8080 \
    DB_PATH=/data/sync.db

VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["rh-sync-server"]
