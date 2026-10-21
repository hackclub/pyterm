FROM rust:1.99.1-trixie AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/pyterm /usr/local/bin/pyterm
USER nobody
EXPOSE 8080
CMD ["pyterm"]
