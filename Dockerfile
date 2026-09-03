# syntax=docker/dockerfile:1

# ---------- desarrollo: recarga en caliente ----------
FROM rust:1.90-bookworm AS dev
WORKDIR /app
RUN cargo install cargo-watch --locked
COPY Cargo.toml ./
COPY migration/Cargo.toml migration/
# Fuentes mínimas para que cargo resuelva y cachee las dependencias
RUN mkdir -p src migration/src \
 && echo 'fn main() {}' > src/main.rs \
 && echo '' > src/lib.rs \
 && echo 'fn main() {}' > migration/src/main.rs \
 && echo '' > migration/src/lib.rs \
 && cargo build 2>/dev/null || true
COPY . .
EXPOSE 8080
# El `touch` no es adorno: arriba se compila un `fn main() {}` falso para cachear las
# dependencias, y ese build queda en /app/target — que en desarrollo es un volumen que
# sobrevive a la reconstrucción de la imagen. Sin esto, cargo compara mtimes, da por fresco
# el fingerprint del binario falso y arranca un main vacío que sale con código 0.
CMD ["sh", "-c", "touch src/main.rs src/lib.rs && cargo watch -x run"]

# ---------- compilación ----------
FROM rust:1.90-bookworm AS builder
WORKDIR /app
COPY Cargo.toml ./
COPY migration/Cargo.toml migration/
RUN mkdir -p src migration/src \
 && echo 'fn main() {}' > src/main.rs \
 && echo '' > src/lib.rs \
 && echo 'fn main() {}' > migration/src/main.rs \
 && echo '' > migration/src/lib.rs \
 && cargo build --release 2>/dev/null || true
COPY . .
# Invalida el caché de los binarios propios sin invalidar el de las dependencias
RUN touch src/main.rs src/lib.rs && cargo build --release --bin jobout-core-api

# ---------- producción ----------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
# Sin root: si alguien escapa del proceso, no escapa a la máquina
RUN useradd --system --create-home --uid 10001 jobout
WORKDIR /app
COPY --from=builder /app/target/release/jobout-core-api /usr/local/bin/jobout-core-api
USER jobout
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
  CMD curl -fsS http://localhost:8080/health || exit 1
CMD ["jobout-core-api"]
