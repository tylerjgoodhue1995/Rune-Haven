FROM rust:1.90-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin veloren-beta-auth

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/veloren-beta-auth /usr/local/bin/veloren-beta-auth
ENV BETA_AUTH_BIND=0.0.0.0:19253
ENV BETA_AUTH_DATA_PATH=/data/auth-state.json
EXPOSE 19253
CMD ["/usr/local/bin/veloren-beta-auth"]
