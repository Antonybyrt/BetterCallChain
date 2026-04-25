FROM rust:1.88 AS builder

WORKDIR /usr/src/app

COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY tools/ ./tools/

RUN cargo build --release -p bcc-visualizer

FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/bcc-visualizer /usr/local/bin/bcc-visualizer

EXPOSE 9090

CMD ["bcc-visualizer", "--bind", "0.0.0.0:9090"]
