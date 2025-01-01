# Builder

FROM rust:slim-bookworm AS builder

WORKDIR /app

COPY . .

RUN cargo build --release --bin scuffle-brawl

# Runtime

FROM debian:bookworm-slim

LABEL org.opencontainers.image.source="https://github.com/scufflecloud/brawl" \
      org.opencontainers.image.description="Scuffle Brawl Bot" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

COPY --from=builder /app/target/release/scuffle-brawl /usr/local/bin/scuffle-brawl

ENTRYPOINT ["/usr/local/bin/scuffle-brawl"]
