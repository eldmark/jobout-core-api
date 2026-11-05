//! Casos de verdad del almacén de archivos (A1–A7 de docs/working/fase2-perfil-cv.md).
//!
//! Se escribieron antes que la implementación, en rojo. La batería compartida (A7) corre
//! idéntica contra el almacén en disco y el de memoria.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use jobout_core_api::blob::{
    new_blob_key, BlobError, BlobStore, LocalBlobStore, MemoryBlobStore, BLOB_DIR_VAR,
};

/// Directorio temporal único que se limpia solo con la API de std al soltarse.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "jobout-blob-test-{}-{}-{n}",
            std::process::id(),
            new_blob_key()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn local(tmp: &TempDir) -> LocalBlobStore {
    LocalBlobStore::new(tmp.path().join("blobs")).unwrap()
}

fn claves_invalidas() -> Vec<String> {
    vec![
        "".into(),
        "..".into(),
        "../secreto".into(),
        "a/b".into(),
        "/etc/passwd".into(),
        "a\\b".into(),
        "a\0b".into(),
        "\0".into(),
        "ABC".into(),
        "Clave".into(),
        "con espacio".into(),
        "a.b".into(),
        "ñandú".into(),
        "a".repeat(65),
        "a".repeat(1000),
    ]
}

/// Batería compartida (A7): el mismo contrato para cualquier implementación.
async fn bateria(store: Arc<dyn BlobStore>) {
    // A1: ida y vuelta, incluidos bytes arbitrarios y nulos, y un archivo vacío.
    let todos: Vec<u8> = (0..=255u8).collect();
    store.put("a1-bytes", &todos).await.unwrap();
    assert_eq!(store.get("a1-bytes").await.unwrap(), todos);
    store.put("a1-vacio", b"").await.unwrap();
    assert_eq!(store.get("a1-vacio").await.unwrap(), Vec::<u8>::new());
    let nulos = vec![0u8; 4096];
    store.put("a1-nulos", &nulos).await.unwrap();
    assert_eq!(store.get("a1-nulos").await.unwrap(), nulos);

    // A1: 5 MB.
    let grande: Vec<u8> = (0..5 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    store.put("a1-grande", &grande).await.unwrap();
    assert!(store.get("a1-grande").await.unwrap() == grande);

    // A1: claves de 1 y de 64 caracteres, con guiones y dígitos.
    store.put("a", b"uno").await.unwrap();
    let larga = format!("{}0-9", "z".repeat(61));
    assert_eq!(larga.len(), 64);
    store.put(&larga, b"larga").await.unwrap();
    assert_eq!(store.get(&larga).await.unwrap(), b"larga");
    let k = new_blob_key();
    store.put(&k, b"uuid").await.unwrap();
    assert_eq!(store.get(&k).await.unwrap(), b"uuid");

    // A2: clave ausente.
    assert!(matches!(
        store.get("no-existe").await,
        Err(BlobError::NotFound)
    ));

    // A3: borrar elimina; borrar otra vez o una ausente no falla.
    store.put("a3", b"x").await.unwrap();
    store.delete("a3").await.unwrap();
    assert!(matches!(store.get("a3").await, Err(BlobError::NotFound)));
    store.delete("a3").await.unwrap();
    store.delete("nunca-existio").await.unwrap();
    // Tras borrar se puede volver a guardar.
    store.put("a3", b"y").await.unwrap();
    assert_eq!(store.get("a3").await.unwrap(), b"y");

    // A4: claves inválidas rechazadas en las tres operaciones.
    for mala in claves_invalidas() {
        assert!(
            matches!(store.put(&mala, b"x").await, Err(BlobError::InvalidKey)),
            "put {mala:?}"
        );
        assert!(
            matches!(store.get(&mala).await, Err(BlobError::InvalidKey)),
            "get {mala:?}"
        );
        assert!(
            matches!(store.delete(&mala).await, Err(BlobError::InvalidKey)),
            "delete {mala:?}"
        );
    }

    // A5: claves distintas no se pisan; la misma clave no se sobrescribe.
    store.put("a5-uno", b"1").await.unwrap();
    store.put("a5-dos", b"2").await.unwrap();
    assert_eq!(store.get("a5-uno").await.unwrap(), b"1");
    assert_eq!(store.get("a5-dos").await.unwrap(), b"2");
    assert!(matches!(
        store.put("a5-uno", b"otro").await,
        Err(BlobError::AlreadyExists)
    ));
    assert_eq!(store.get("a5-uno").await.unwrap(), b"1");

    // Los mensajes de error no filtran contenido ni rutas.
    for e in [
        BlobError::NotFound,
        BlobError::AlreadyExists,
        BlobError::InvalidKey,
        BlobError::Io,
    ] {
        let t = format!("{e} {e:?}");
        assert!(!t.contains('/') && !t.contains("tmp"), "{t}");
    }

    // Concurrencia: exactamente un put gana.
    let mut tareas = Vec::new();
    for i in 0..8u8 {
        let s = store.clone();
        tareas.push(tokio::spawn(
            async move { s.put("carrera", &[i; 1024]).await },
        ));
    }
    let mut ok = 0;
    let mut existe = 0;
    for t in tareas {
        match t.await.unwrap() {
            Ok(()) => ok += 1,
            Err(BlobError::AlreadyExists) => existe += 1,
            Err(e) => panic!("error inesperado: {e}"),
        }
    }
    assert_eq!((ok, existe), (1, 7));
    let guardado = store.get("carrera").await.unwrap();
    assert_eq!(guardado.len(), 1024);
    assert!(
        guardado.iter().all(|b| *b == guardado[0]),
        "mezcla de escrituras"
    );
}

/// A7 contra el disco.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bateria_compartida_en_disco() {
    let tmp = TempDir::new();
    bateria(Arc::new(local(&tmp))).await;
}

/// A7 contra la memoria.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bateria_compartida_en_memoria() {
    bateria(Arc::new(MemoryBlobStore::new())).await;
}

/// A6: el directorio base es 0700 y los archivos 0600.
#[tokio::test]
async fn permisos_del_directorio_y_de_los_archivos() {
    let tmp = TempDir::new();
    let base = tmp.path().join("blobs");
    let s = LocalBlobStore::new(&base).unwrap();
    s.put("a6", b"secreto").await.unwrap();
    let modo_dir = std::fs::metadata(&base).unwrap().permissions().mode() & 0o777;
    assert_eq!(modo_dir, 0o700);
    let modo_arch = std::fs::metadata(base.join("a6"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(modo_arch, 0o600);
}

/// A6: un directorio base anidado también queda 0700 en lo que se crea.
#[tokio::test]
async fn directorio_anidado_se_crea_privado() {
    let tmp = TempDir::new();
    let base = tmp.path().join("x").join("y").join("blobs");
    LocalBlobStore::new(&base).unwrap();
    let modo = std::fs::metadata(&base).unwrap().permissions().mode() & 0o777;
    assert_eq!(modo, 0o700);
}

/// Un directorio base que ya existe se reutiliza sin romper los datos previos.
#[tokio::test]
async fn reabrir_el_directorio_conserva_los_datos() {
    let tmp = TempDir::new();
    let base = tmp.path().join("blobs");
    LocalBlobStore::new(&base)
        .unwrap()
        .put("persistente", b"hola")
        .await
        .unwrap();
    let otra = LocalBlobStore::new(&base).unwrap();
    assert_eq!(otra.get("persistente").await.unwrap(), b"hola");
}

/// A4: una clave maliciosa no toca el disco: no crea nada fuera ni dentro del directorio base.
#[tokio::test]
async fn clave_maliciosa_no_toca_el_disco() {
    let tmp = TempDir::new();
    let s = local(&tmp);
    // Un archivo "secreto" junto al directorio base.
    let fuera = tmp.path().join("secreto");
    std::fs::write(&fuera, b"no tocar").unwrap();

    assert!(matches!(
        s.put("../secreto", b"pisado").await,
        Err(BlobError::InvalidKey)
    ));
    assert!(matches!(
        s.get("../secreto").await,
        Err(BlobError::InvalidKey)
    ));
    assert!(matches!(
        s.delete("../secreto").await,
        Err(BlobError::InvalidKey)
    ));
    assert_eq!(std::fs::read(&fuera).unwrap(), b"no tocar");
    assert_eq!(
        std::fs::read_dir(tmp.path().join("blobs")).unwrap().count(),
        0
    );
}

/// Un enlace simbólico dentro del directorio no permite leer fuera de él.
#[tokio::test]
async fn enlace_simbolico_no_se_sigue_al_leer() {
    let tmp = TempDir::new();
    let s = local(&tmp);
    let fuera = tmp.path().join("secreto");
    std::fs::write(&fuera, b"no tocar").unwrap();
    std::os::unix::fs::symlink(&fuera, tmp.path().join("blobs").join("enlace")).unwrap();
    assert!(s.get("enlace").await.is_err());
    assert!(matches!(
        s.put("enlace", b"x").await,
        Err(BlobError::AlreadyExists)
    ));
    assert_eq!(std::fs::read(&fuera).unwrap(), b"no tocar");
}

/// Escritura atómica: tras un put correcto o fallido no quedan temporales en el directorio.
#[tokio::test]
async fn no_quedan_archivos_temporales() {
    let tmp = TempDir::new();
    let s = local(&tmp);
    s.put("uno", b"1").await.unwrap();
    let _ = s.put("uno", b"2").await; // AlreadyExists
    let mut nombres: Vec<String> = std::fs::read_dir(tmp.path().join("blobs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    nombres.sort();
    assert_eq!(nombres, vec!["uno".to_string()]);
}

/// Un put sobre una clave existente no altera el archivo ya guardado (ni parcialmente).
#[tokio::test]
async fn put_fallido_no_deja_el_archivo_alterado() {
    let tmp = TempDir::new();
    let s = local(&tmp);
    s.put("fijo", b"original").await.unwrap();
    assert!(s.put("fijo", &vec![9u8; 1 << 20]).await.is_err());
    assert_eq!(s.get("fijo").await.unwrap(), b"original");
}

/// Si el directorio base no se puede crear, el error no revela la ruta.
#[test]
fn error_al_crear_el_directorio_no_revela_la_ruta() {
    let tmp = TempDir::new();
    let archivo = tmp.path().join("soy-un-archivo");
    std::fs::write(&archivo, b"x").unwrap();
    let err = LocalBlobStore::new(archivo.join("dentro")).err().unwrap();
    let texto = format!("{err} {err:?}");
    assert!(!texto.contains("soy-un-archivo"), "{texto}");
    assert!(!texto.contains(tmp.path().to_str().unwrap()), "{texto}");
}

fn lookup_con(valor: Option<&str>) -> impl Fn(&str) -> Option<String> {
    let v = valor.map(str::to_string);
    move |n| if n == BLOB_DIR_VAR { v.clone() } else { None }
}

/// `from_lookup` usa BLOB_DIR.
#[tokio::test]
async fn from_lookup_usa_blob_dir() {
    let tmp = TempDir::new();
    let base = tmp.path().join("desde-env");
    let s = LocalBlobStore::from_lookup(lookup_con(Some(base.to_str().unwrap()))).unwrap();
    s.put("k", b"v").await.unwrap();
    assert!(base.join("k").exists());
}

/// Falta la variable: el error nombra la variable.
#[test]
fn from_lookup_sin_variable_nombra_la_variable() {
    let err = LocalBlobStore::from_lookup(lookup_con(None)).err().unwrap();
    assert!(err.to_string().contains("BLOB_DIR"));
    assert_eq!(BLOB_DIR_VAR, "BLOB_DIR");
}

/// Variable vacía o en blanco se trata como ausente.
#[test]
fn from_lookup_variable_vacia_es_error() {
    for v in ["", "   "] {
        let err = LocalBlobStore::from_lookup(lookup_con(Some(v)))
            .err()
            .unwrap();
        assert!(err.to_string().contains("BLOB_DIR"));
    }
}

/// El valor de la variable nunca aparece en el error ni en Debug.
#[test]
fn from_lookup_no_imprime_el_valor() {
    let tmp = TempDir::new();
    let archivo = tmp.path().join("valor-sensible");
    std::fs::write(&archivo, b"x").unwrap();
    let valor = archivo.join("dentro");
    let err = LocalBlobStore::from_lookup(lookup_con(Some(valor.to_str().unwrap())))
        .err()
        .unwrap();
    let t = format!("{err} {err:?}");
    assert!(!t.contains("valor-sensible"), "{t}");
}

/// Debug del almacén en disco no imprime la ruta.
#[test]
fn debug_del_almacen_no_imprime_la_ruta() {
    let tmp = TempDir::new();
    let s = local(&tmp);
    assert!(!format!("{s:?}").contains("jobout-blob-test"));
}

/// Claves nuevas: UUID v7 en minúsculas, únicas, válidas para el almacén.
#[tokio::test]
async fn new_blob_key_es_uuid_v7_minusculas_y_unica() {
    let mut vistas = HashSet::new();
    for _ in 0..1000 {
        let k = new_blob_key();
        assert_eq!(k.len(), 36);
        assert_eq!(k, k.to_lowercase());
        let u = uuid::Uuid::parse_str(&k).unwrap();
        assert_eq!(u.get_version_num(), 7);
        assert!(vistas.insert(k));
    }
    let k = new_blob_key();
    MemoryBlobStore::new().put(&k, b"x").await.unwrap();
}
