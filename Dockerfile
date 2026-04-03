FROM rust:1.85 AS builder

WORKDIR /usr/src/app

# Copy the entire workspace
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

# Build both node and client
RUN cargo build --release --workspace

# Runtime image for both node and client interactions
FROM debian:bookworm-slim

WORKDIR /app

# Install required dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    jq \
    && rm -rf /var/lib/apt/lists/*

# Copy binaries
COPY --from=builder /usr/src/app/target/release/bcc-node /usr/local/bin/bcc-node
COPY --from=builder /usr/src/app/target/release/bcc-client /usr/local/bin/bcc-client

# Create directories for config and data
RUN mkdir -p /app/config /data

# --config <path> is required; pass it via docker-compose or docker run command.
CMD ["bcc-node"]
