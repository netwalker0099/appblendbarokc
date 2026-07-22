# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS build
WORKDIR /app
COPY api/Cargo.toml api/Cargo.lock* ./
COPY api/src ./src
COPY api/migrations ./migrations
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/blendbar-api /usr/local/bin/blendbar-api
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=5 \
    CMD curl -f http://localhost:8080/api/health || exit 1
CMD ["blendbar-api"]
