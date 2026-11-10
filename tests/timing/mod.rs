//! Medición de costo para las pruebas que comparan trabajo (no relojes de pared).
//!
//! Un reloj de pared mide también lo que hacen los demás procesos: bajo carga de CPU la misma
//! función puede tardar 10 veces más sin que su algoritmo haya cambiado. Aquí se mide el
//! **tiempo de CPU del hilo** (lo que el hilo realmente ejecutó), que no crece porque el
//! planificador lo haya dejado esperando, y se toma el **mínimo** de varias repeticiones para
//! quitar lo que sí queda (caché fría, contención de memoria).

#![allow(dead_code)]

use std::time::Duration;

/// Tiempo de CPU consumido por el hilo actual. En plataformas sin ese reloj cae al de pared.
#[cfg(target_os = "linux")]
pub fn cpu_now() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` es un `timespec` válido y vive durante toda la llamada.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    assert_eq!(rc, 0, "clock_gettime(CLOCK_THREAD_CPUTIME_ID) falló");
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

#[cfg(not(target_os = "linux"))]
pub fn cpu_now() -> Duration {
    use std::sync::OnceLock;
    use std::time::Instant;
    static ORIGEN: OnceLock<Instant> = OnceLock::new();
    ORIGEN.get_or_init(Instant::now).elapsed()
}

/// Lo que cuesta (en CPU del hilo) ejecutar `f` una vez.
pub fn costo<R>(f: impl FnOnce() -> R) -> Duration {
    let t0 = cpu_now();
    std::hint::black_box(f());
    cpu_now().saturating_sub(t0)
}

/// El menor costo de `intentos` ejecuciones de `f`.
pub fn costo_minimo(intentos: usize, mut f: impl FnMut()) -> Duration {
    (0..intentos)
        .map(|_| costo(&mut f))
        .min()
        .expect("al menos un intento")
}

/// Cuánto más cuesta `t_grande` que `t_chico`. El divisor se acota por abajo para que un
/// tiempo casi nulo (menor que la resolución del reloj) no dispare el cociente.
pub fn ratio(t_chico: Duration, t_grande: Duration, piso: Duration) -> f64 {
    t_grande.as_secs_f64() / t_chico.max(piso).as_secs_f64()
}
