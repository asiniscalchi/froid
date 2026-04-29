FROM rust:1-bookworm AS builder

WORKDIR /app
ARG FROID_VERSION=unknown
ENV FROID_VERSION=${FROID_VERSION}

RUN apt-get update \
    && apt-get install -y --no-install-recommends libsqlite3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY migrations ./migrations
COPY src ./src

RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime
ARG FROID_VERSION=unknown
ENV FROID_VERSION=${FROID_VERSION}
LABEL org.opencontainers.image.revision=${FROID_VERSION}

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/froid /usr/local/bin/froid

ENTRYPOINT ["/usr/local/bin/froid"]
