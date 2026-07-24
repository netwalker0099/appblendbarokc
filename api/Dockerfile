# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS build
WORKDIR /app
COPY api/Cargo.toml api/Cargo.lock* ./
COPY api/src ./src
COPY api/migrations ./migrations
RUN cargo build --release

FROM debian:bookworm-slim
# curl for the healthcheck; postgresql-client-16 (from PGDG, matching the PG16
# server) provides pg_dump for the admin backup-download endpoint.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc -o /etc/apt/trusted.gpg.d/pgdg.asc \
    && echo "deb https://apt.postgresql.org/pub/repos/apt bookworm-pgdg main" > /etc/apt/sources.list.d/pgdg.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends postgresql-client-16 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/blendbar-api /usr/local/bin/blendbar-api
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=5 \
    CMD curl -f http://localhost:8080/api/health || exit 1
CMD ["blendbar-api"]
