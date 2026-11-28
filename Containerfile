FROM docker.io/library/rust:1.99.1-bookworm AS build

WORKDIR /src

# Dependency layer: compile third-party crates against stub sources so this
# layer is reused until Cargo.toml or Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
COPY server/Cargo.toml server/Cargo.toml
COPY crates/example/Cargo.toml crates/example/Cargo.toml
COPY crates/extraction/Cargo.toml crates/extraction/Cargo.toml
RUN mkdir server/src crates/example/src crates/extraction/src \
    && echo 'fn main() {}' > server/src/main.rs \
    && touch crates/example/src/lib.rs crates/extraction/src/lib.rs \
    && cargo build -p paradis-server --release --locked

# Cargo decides freshness by mtime and COPY keeps the context's (older)
# timestamps, so touch the sources to force a rebuild over the stubs.
COPY . .
RUN find server/src crates/example/src crates/extraction/src -type f -name '*.rs' -exec touch {} + \
    && cargo build -p paradis-server --release --locked \
    && strip target/release/paradis-server

FROM docker.io/library/debian:bookworm-slim

RUN groupadd --system paradis \
    && useradd --system --gid paradis --no-create-home --shell /usr/sbin/nologin paradis \
    && mkdir -p /data \
    && chown paradis:paradis /data

WORKDIR /app
COPY --from=build --chown=paradis:paradis /src/target/release/paradis-server /app/paradis-server
COPY --from=build --chown=paradis:paradis /src/server/config /app/server/config

ENV APP_ENVIRONMENT=prd \
    SQLITE_DSN=/data/paradis.db

EXPOSE 8080
VOLUME ["/data"]
USER paradis
ENTRYPOINT ["/app/paradis-server"]
