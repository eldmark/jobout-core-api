//! Registro explícito de rutas (S5): cuáles son públicas, con por qué, y cuáles exigen `AuthUser`.
//!
//! Axum no permite listar las rutas de un `Router`, así que la lista se escribe a mano aquí y
//! `tests/routes_registry.rs` la cruza con el código fuente (toda ruta montada tiene que estar
//! declarada) y con el comportamiento (cada ruta protegida da 401 sin token).
//!
//! Al montar una ruta nueva hay que añadirla a **una** de las dos listas. Si es pública, hay
//! que escribir por qué.

/// Ruta que se puede llamar sin token, y la razón.
pub struct PublicRoute {
    pub path: &'static str,
    pub methods: &'static [&'static str],
    pub reason: &'static str,
}

/// Ruta que exige `AuthUser`.
pub struct ProtectedRoute {
    pub path: &'static str,
    pub methods: &'static [&'static str],
}

pub const PUBLIC: &[PublicRoute] = &[
    PublicRoute {
        path: "/health",
        methods: &["GET"],
        reason: "Healthcheck de Docker y del orquestador; no devuelve datos de usuario.",
    },
    PublicRoute {
        path: "/api/v1/auth/register",
        methods: &["POST"],
        reason: "Puerta de entrada: crea la cuenta, todavía no hay token.",
    },
    PublicRoute {
        path: "/api/v1/auth/login",
        methods: &["POST"],
        reason: "Puerta de entrada: entrega el primer token a partir de la contraseña.",
    },
    PublicRoute {
        path: "/api/v1/auth/refresh",
        methods: &["POST"],
        reason: "Se llama justo porque el access token venció; se autentica con el refresh token.",
    },
    PublicRoute {
        path: "/api/v1/auth/logout",
        methods: &["POST"],
        reason: "Se autentica con el refresh token que revoca; debe poder cerrarse con el access vencido.",
    },
];

pub const PROTECTED: &[ProtectedRoute] = &[ProtectedRoute {
    path: "/api/v1/me",
    methods: &["GET", "DELETE"],
}];
