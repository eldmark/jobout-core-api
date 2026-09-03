# jobout-core-api

El core de **JobOut**. Rust · Axum · SeaORM · PostgreSQL.

Único servicio con acceso a la base de datos y único que maneja PII: auth, perfiles, CVs,
ofertas, aplicaciones, sesiones de entrevista y scores.

Documentación del proyecto: **[jobout-docs](https://github.com/eldmark/jobout-docs)**.
Reglas para agentes: [`AGENTS.md`](AGENTS.md).

## Levantar

```bash
cp .env.example .env                              # completá los valores
cp docker-compose.example.yml docker-compose.yml
docker compose up
```

```
http://localhost:8080/health
```

`docker-compose.yml` y `.env` están en `.gitignore`: son tuyos, tocalos sin miedo. Lo que se
versiona son los `.example`.

**Backend y frontend juntos, con un comando:**

```bash
cd ../jobout-docs && make dev
```

Requiere que los tres repos estén clonados como hermanos:

```
jobout/
├── jobout-docs/
├── jobout-core-api/     ← este
└── jobout-web/
```

## Desarrollar

```bash
cargo test                                 # pruebas
cargo clippy --all-targets -- -D warnings  # lint
cargo fmt --all                            # formato
cargo run                                  # sin Docker (necesita PostgreSQL y rustc >= 1.88)
```

El contenedor de desarrollo usa `cargo-watch`: al guardar un archivo, recompila y reinicia.

## Migraciones

```bash
docker compose run --rm core-api cargo run --bin migration -- up
docker compose run --rm core-api cargo run --bin migration -- down -n 1
docker compose run --rm core-api cargo run --bin migration -- generate nombre_de_la_migracion
```

El esquema está diseñado en `jobout-docs/documentation/db.md`. **Toda migración implementa
`up` y `down`, y actualiza ese documento en el mismo PR** — el CI lo verifica.

## Estructura

```
src/
  main.rs          arranque del servidor y apagado ordenado
  lib.rs           app() — el router, aparte de main para poder probarlo
  routes/          una ruta por archivo
migration/         crate de migraciones SeaORM
tests/             pruebas de integración
```

`app()` vive en `lib.rs` y no en `main.rs` a propósito: las pruebas de integración levantan la
aplicación completa sin abrir un puerto.
