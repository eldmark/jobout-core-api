# AGENTS.md — jobout-core-api

**Las reglas del proyecto viven en [`jobout-docs`](https://github.com/eldmark/jobout-docs).**
Este archivo es el resumen; ante cualquier duda, manda `jobout-docs/AGENTS.md`.

## Qué es este servicio

El **core** de JobOut. Rust · Axum · SeaORM · PostgreSQL.

Es el único servicio que habla con la base de datos y el único que ve PII. Auth, perfiles, CVs,
ofertas, aplicaciones, sesiones de entrevista, scores y pagos.

## Antes de tocar nada

1. `jobout-docs/docs/continue.md` — qué se hizo y qué sigue
2. `jobout-docs/docs/bugs.md` — bugs abiertos
3. `jobout-docs/documentation/db.md` — el esquema
4. `git status --short` acá — ¿hay cambios que no son tuyos?

## Reglas no negociables

1. **Nunca** commitees `.env`, claves, tokens ni `docker-compose.yml`. Secreto en el diff = se
   para todo y se rota la credencial
2. **Cambio de esquema ⇒ migración SeaORM + `jobout-docs/documentation/db.md` en el mismo PR.**
   Toda migración implementa `up` **y** `down`
3. **Los casos de verdad se acuerdan antes de codear**, en el archivo de fase. Después el test
   en rojo, después el código
4. Cada función nueva nace con su prueba unitaria
5. Cada endpoint nace con su prueba de integración
6. PII cifrada a nivel de aplicación antes de tocar la BD. **Nunca en logs**
7. **Toda ruta nueva nace autenticada.** Si es pública, va el comentario que lo justifica —
   como en `src/routes/health.rs`
8. **Si un listado filtra por dueño, la consulta por id filtra igual.** Es la falla de
   autorización más común
9. Nada de SQL con interpolación de parámetros de ruta
10. **A `jobout-interview` no le mandes PII.** Si te encontrás pasándole el email del usuario,
    la solución no es cifrarlo: es no pasarlo

## Cómo correr

```bash
cp .env.example .env && cp docker-compose.example.yml docker-compose.yml
docker compose up                 # solo este servicio
cargo test                        # pruebas
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Backend + frontend juntos: `cd ../jobout-docs && make dev`

## Commits y PRs

- Rama `jb-##-slug`. El CI rechaza otro formato
- Mensajes **en inglés**, sin atribución de IA de ningún tipo
- Las cuatro secciones del PR son obligatorias
- Squash merge, una aprobación, CI verde

## Al terminar

- `cargo test` y `cargo clippy -- -D warnings` en verde
- `jobout-docs/docs/continue.md` actualizado
- Si cambiaste el esquema, `documentation/db.md` también
