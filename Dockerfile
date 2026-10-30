# SPDX-License-Identifier: AGPL-3.0-or-later
# La API en una imagen pequeña: se compila en una etapa y solo el binario pasa a la final. Las migraciones y la malla
# del pronóstico van dentro del binario (sqlx::migrate! e include_str!), así que no hace falta copiar nada más.

FROM rust:1-slim-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY src src
COPY migrations migrations
COPY data data
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 illapa
COPY --from=build /app/target/release/illapa-engine /usr/local/bin/illapa-engine
USER illapa
ENV ILLAPA_ADDR=0.0.0.0:8080
EXPOSE 8080
CMD ["illapa-engine"]
