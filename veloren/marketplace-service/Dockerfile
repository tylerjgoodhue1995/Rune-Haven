FROM rust:1.90-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p veloren-marketplace-service

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/veloren-marketplace-service /usr/local/bin/veloren-marketplace-service
EXPOSE 19254
ENTRYPOINT ["/usr/local/bin/veloren-marketplace-service"]
