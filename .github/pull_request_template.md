<!--
Título del PR:  jb-##-implementation — qué hace en pocas palabras
Las cuatro secciones son obligatorias. Si una no aplica, se escribe "Ninguna" — no se borra.
Guía completa: CONTRIBUTING.md
-->

## Qué se hizo

<!-- En prosa, para alguien que no siguió la tarea. El qué y el para qué, no el cómo. -->

Cierra: #

Historias de usuario que cubre:
Fase: `docs/working/faseN-....md`

## Cómo se resolvió

<!--
Las decisiones que tomaste y por qué. Lo que descartaste y por qué.
Si hay un rodeo feo, decilo acá: el reviewer lo va a encontrar igual.
-->

## Archivos tocados

<!-- `git diff --name-status main` te da la lista. La explicación la escribís vos. -->

| Archivo | Qué le pasó |
|---|---|
| | |

## Migraciones

<!--
Si hay cambio de esquema, esta sección NO puede quedar vacía.
Si no hay, escribí "Ninguna".
-->

**Ninguna**

<!-- Si hay, borrá lo de arriba y completá:

| | |
|---|---|
| Migración | `mYYYYMMDD_HHMMSS_nombre` |
| Tablas | |
| Columnas | |
| Reversible | Sí — `down` implementado y probado |
| Al desplegar | Nada especial / requiere backfill / requiere ventana |

`documentation/db.md` actualizado en este mismo PR: [ ] sí
-->

---

## Casos de verdad

<!--
Los que se acordaron en el archivo de fase ANTES de implementar.
Uno por línea, cada uno con el test que lo cubre.
-->

| Caso | Test |
|---|---|
| | |

## Antes de pedir revisión

- [ ] Los tests fallaron primero y ahora pasan
- [ ] Cada función nueva tiene prueba unitaria
- [ ] Cada endpoint o flujo entre servicios tiene prueba de integración
- [ ] CI en verde
- [ ] **Ningún secreto en el diff** — ni `.env`, ni claves, ni tokens
- [ ] Ningún dato personal en logs ni en mensajes de error
- [ ] Toda ruta nueva nace autenticada, o lleva el comentario que justifica ser pública
- [ ] Si hay migración, `documentation/db.md` va actualizado acá
- [ ] La pantalla abierta en un navegador, si toqué frontend
- [ ] `docs/continue.md` actualizado
