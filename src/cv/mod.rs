//! Lectura de CVs: texto (PDF y DOCX) y extractor determinista de propuestas (Fase 2, JB-21).
//!
//! Son módulos puros: sin base de datos, sin red, sin rutas. Reciben bytes o texto y devuelven
//! valores o errores de dominio.

pub mod extractor;
pub mod text;
