//! Semillas de los catálogos de ofertas (Fase 5, paso 1).

/// Un departamento: (code, nombre, municipios).
pub type Departamento = (&'static str, &'static str, &'static [&'static str]);

/// Los 22 departamentos de Guatemala con sus 340 municipios.
pub const GT_DEPARTAMENTOS: &[Departamento] = &[
    (
        "alta-verapaz",
        "Alta Verapaz",
        &[
            "Cobán",
            "Santa Cruz Verapaz",
            "San Cristóbal Verapaz",
            "Tactic",
            "Tamahú",
            "Tucurú",
            "Panzós",
            "Senahú",
            "San Pedro Carchá",
            "San Juan Chamelco",
            "Lanquín",
            "Cahabón",
            "Chisec",
            "Chahal",
            "Fray Bartolomé de las Casas",
            "Santa Catalina La Tinta",
            "Raxruhá",
        ],
    ),
    (
        "baja-verapaz",
        "Baja Verapaz",
        &[
            "Salamá",
            "San Miguel Chicaj",
            "Rabinal",
            "Cubulco",
            "Granados",
            "El Chol",
            "San Jerónimo",
            "Purulhá",
        ],
    ),
    (
        "chimaltenango",
        "Chimaltenango",
        &[
            "Chimaltenango",
            "San José Poaquil",
            "San Martín Jilotepeque",
            "San Juan Comalapa",
            "Santa Apolonia",
            "Tecpán Guatemala",
            "Patzún",
            "Pochuta",
            "Patzicía",
            "Santa Cruz Balanyá",
            "Acatenango",
            "Yepocapa",
            "San Andrés Itzapa",
            "Parramos",
            "Zaragoza",
            "El Tejar",
        ],
    ),
    (
        "chiquimula",
        "Chiquimula",
        &[
            "Chiquimula",
            "San José La Arada",
            "San Juan Ermita",
            "Jocotán",
            "Camotán",
            "Olopa",
            "Esquipulas",
            "Concepción Las Minas",
            "Quezaltepeque",
            "San Jacinto",
            "Ipala",
        ],
    ),
    (
        "el-progreso",
        "El Progreso",
        &[
            "Guastatoya",
            "Morazán",
            "San Agustín Acasaguastlán",
            "San Cristóbal Acasaguastlán",
            "El Jícaro",
            "Sansare",
            "Sanarate",
            "San Antonio La Paz",
        ],
    ),
    (
        "escuintla",
        "Escuintla",
        &[
            "Escuintla",
            "Santa Lucía Cotzumalguapa",
            "La Democracia",
            "Siquinalá",
            "Masagua",
            "Tiquisate",
            "La Gomera",
            "Guanagazapa",
            "San José",
            "Iztapa",
            "Palín",
            "San Vicente Pacaya",
            "Nueva Concepción",
            "Sipacate",
        ],
    ),
    (
        "guatemala",
        "Guatemala",
        &[
            "Guatemala",
            "Santa Catarina Pinula",
            "San José Pinula",
            "San José del Golfo",
            "Palencia",
            "Chinautla",
            "San Pedro Ayampuc",
            "Mixco",
            "San Pedro Sacatepéquez",
            "San Juan Sacatepéquez",
            "San Raymundo",
            "Chuarrancho",
            "Fraijanes",
            "Amatitlán",
            "Villa Nueva",
            "Villa Canales",
            "San Miguel Petapa",
        ],
    ),
    (
        "huehuetenango",
        "Huehuetenango",
        &[
            "Huehuetenango",
            "Chiantla",
            "Malacatancito",
            "Cuilco",
            "Nentón",
            "San Pedro Necta",
            "Jacaltenango",
            "San Pedro Soloma",
            "San Ildefonso Ixtahuacán",
            "Santa Bárbara",
            "La Libertad",
            "La Democracia",
            "San Miguel Acatán",
            "San Rafael La Independencia",
            "Todos Santos Cuchumatán",
            "San Juan Atitán",
            "Santa Eulalia",
            "San Mateo Ixtatán",
            "Colotenango",
            "San Sebastián Huehuetenango",
            "Tectitán",
            "Concepción Huista",
            "San Juan Ixcoy",
            "San Antonio Huista",
            "San Sebastián Coatán",
            "Santa Cruz Barillas",
            "Aguacatán",
            "San Rafael Petzal",
            "San Gaspar Ixchil",
            "Santiago Chimaltenango",
            "Santa Ana Huista",
            "Unión Cantinil",
            "Petatán",
        ],
    ),
    (
        "izabal",
        "Izabal",
        &[
            "Puerto Barrios",
            "Livingston",
            "El Estor",
            "Morales",
            "Los Amates",
        ],
    ),
    (
        "jalapa",
        "Jalapa",
        &[
            "Jalapa",
            "San Pedro Pinula",
            "San Luis Jilotepeque",
            "San Manuel Chaparrón",
            "San Carlos Alzatate",
            "Monjas",
            "Mataquescuintla",
        ],
    ),
    (
        "jutiapa",
        "Jutiapa",
        &[
            "Jutiapa",
            "El Progreso",
            "Santa Catarina Mita",
            "Agua Blanca",
            "Asunción Mita",
            "Yupiltepeque",
            "Atescatempa",
            "Jerez",
            "El Adelanto",
            "Zapotitlán",
            "Comapa",
            "Jalpatagua",
            "Conguaco",
            "Moyuta",
            "Pasaco",
            "San José Acatempa",
            "Quesada",
        ],
    ),
    (
        "peten",
        "Petén",
        &[
            "Flores",
            "San José",
            "San Benito",
            "San Andrés",
            "La Libertad",
            "San Francisco",
            "Santa Ana",
            "Dolores",
            "San Luis",
            "Sayaxché",
            "Melchor de Mencos",
            "Poptún",
            "Las Cruces",
            "El Chal",
        ],
    ),
    (
        "quetzaltenango",
        "Quetzaltenango",
        &[
            "Quetzaltenango",
            "Salcajá",
            "Olintepeque",
            "San Carlos Sija",
            "Sibilia",
            "Cabricán",
            "Cajolá",
            "San Miguel Sigüilá",
            "San Juan Ostuncalco",
            "San Mateo",
            "Concepción Chiquirichapa",
            "San Martín Sacatepéquez",
            "Almolonga",
            "Cantel",
            "Huitán",
            "Zunil",
            "Colomba",
            "San Francisco La Unión",
            "El Palmar",
            "Coatepeque",
            "Génova",
            "Flores Costa Cuca",
            "La Esperanza",
            "Palestina de Los Altos",
        ],
    ),
    (
        "quiche",
        "Quiché",
        &[
            "Santa Cruz del Quiché",
            "Chiché",
            "Chinique",
            "Zacualpa",
            "Chajul",
            "Chichicastenango",
            "Patzité",
            "San Antonio Ilotenango",
            "San Pedro Jocopilas",
            "Cunén",
            "San Juan Cotzal",
            "Joyabaj",
            "Nebaj",
            "San Andrés Sajcabajá",
            "Uspantán",
            "Sacapulas",
            "San Bartolomé Jocotenango",
            "Canillá",
            "Chicamán",
            "Ixcán",
            "Pachalum",
        ],
    ),
    (
        "retalhuleu",
        "Retalhuleu",
        &[
            "Retalhuleu",
            "San Sebastián",
            "Santa Cruz Muluá",
            "San Martín Zapotitlán",
            "San Felipe",
            "San Andrés Villa Seca",
            "Champerico",
            "Nuevo San Carlos",
            "El Asintal",
        ],
    ),
    (
        "sacatepequez",
        "Sacatepéquez",
        &[
            "Antigua Guatemala",
            "Jocotenango",
            "Pastores",
            "Sumpango",
            "Santo Domingo Xenacoj",
            "Santiago Sacatepéquez",
            "San Bartolomé Milpas Altas",
            "San Lucas Sacatepéquez",
            "Santa Lucía Milpas Altas",
            "Magdalena Milpas Altas",
            "Santa María de Jesús",
            "Ciudad Vieja",
            "San Miguel Dueñas",
            "Alotenango",
            "San Antonio Aguas Calientes",
            "Santa Catarina Barahona",
        ],
    ),
    (
        "san-marcos",
        "San Marcos",
        &[
            "San Marcos",
            "San Pedro Sacatepéquez",
            "San Antonio Sacatepéquez",
            "Comitancillo",
            "San Miguel Ixtahuacán",
            "Concepción Tutuapa",
            "Tacaná",
            "Sibinal",
            "Tajumulco",
            "Tejutla",
            "San Rafael Pie de la Cuesta",
            "Nuevo Progreso",
            "El Tumbador",
            "San José El Rodeo",
            "Malacatán",
            "Catarina",
            "Ayutla",
            "Ocós",
            "San Pablo",
            "El Quetzal",
            "La Reforma",
            "Pajapita",
            "Ixchiguán",
            "San José Ojetenam",
            "San Cristóbal Cucho",
            "Sipacapa",
            "Esquipulas Palo Gordo",
            "Río Blanco",
            "San Lorenzo",
            "La Blanca",
        ],
    ),
    (
        "santa-rosa",
        "Santa Rosa",
        &[
            "Cuilapa",
            "Barberena",
            "Santa Rosa de Lima",
            "Casillas",
            "San Rafael Las Flores",
            "Oratorio",
            "San Juan Tecuaco",
            "Chiquimulilla",
            "Taxisco",
            "Santa María Ixhuatán",
            "Guazacapán",
            "Santa Cruz Naranjo",
            "Pueblo Nuevo Viñas",
            "Nueva Santa Rosa",
        ],
    ),
    (
        "solola",
        "Sololá",
        &[
            "Sololá",
            "San José Chacayá",
            "Santa María Visitación",
            "Santa Lucía Utatlán",
            "Nahualá",
            "Santa Catarina Ixtahuacán",
            "Santa Clara La Laguna",
            "Concepción",
            "San Andrés Semetabaj",
            "Panajachel",
            "Santa Catarina Palopó",
            "San Antonio Palopó",
            "San Lucas Tolimán",
            "Santa Cruz La Laguna",
            "San Pablo La Laguna",
            "San Marcos La Laguna",
            "San Juan La Laguna",
            "San Pedro La Laguna",
            "Santiago Atitlán",
        ],
    ),
    (
        "suchitepequez",
        "Suchitepéquez",
        &[
            "Mazatenango",
            "Cuyotenango",
            "San Francisco Zapotitlán",
            "San Bernardino",
            "San José El Ídolo",
            "Santo Domingo Suchitepéquez",
            "San Lorenzo",
            "Samayac",
            "San Pablo Jocopilas",
            "San Antonio Suchitepéquez",
            "San Miguel Panán",
            "San Gabriel",
            "Chicacao",
            "Patulul",
            "Santa Bárbara",
            "San Juan Bautista",
            "Santo Tomás La Unión",
            "Zunilito",
            "Pueblo Nuevo",
            "Río Bravo",
            "San José La Máquina",
        ],
    ),
    (
        "totonicapan",
        "Totonicapán",
        &[
            "Totonicapán",
            "San Cristóbal Totonicapán",
            "San Francisco El Alto",
            "San Andrés Xecul",
            "Momostenango",
            "Santa María Chiquimula",
            "Santa Lucía La Reforma",
            "San Bartolo",
        ],
    ),
    (
        "zacapa",
        "Zacapa",
        &[
            "Zacapa",
            "Estanzuela",
            "Río Hondo",
            "Gualán",
            "Teculután",
            "Usumatlán",
            "Cabañas",
            "San Diego",
            "La Unión",
            "Huité",
            "San Jorge",
        ],
    ),
];

/// Rangos salariales mensuales en quetzales: (code, nombre, mínimo, máximo). Mínimo y máximo
/// son inclusivos y en quetzales enteros; el último no tiene tope.
pub type Rango = (&'static str, &'static str, i64, Option<i64>);
pub const SALARY_RANGES: &[Rango] = &[
    ("lt-3000", "Menos de Q3,000", 0, Some(2_999)),
    ("3000-4999", "Q3,000 a Q4,999", 3_000, Some(4_999)),
    ("5000-7999", "Q5,000 a Q7,999", 5_000, Some(7_999)),
    ("8000-11999", "Q8,000 a Q11,999", 8_000, Some(11_999)),
    ("12000-19999", "Q12,000 a Q19,999", 12_000, Some(19_999)),
    ("20000-34999", "Q20,000 a Q34,999", 20_000, Some(34_999)),
    ("gte-35000", "Q35,000 o más", 35_000, None),
];

/// Jornadas de trabajo (Código de Trabajo: diurna, mixta y nocturna) y las formas comunes de
/// repartir las horas.
pub const WORKING_SHIFTS: &[(&str, &str)] = &[
    ("diurna", "Diurna"),
    ("mixta", "Mixta"),
    ("nocturna", "Nocturna"),
    ("rotativa", "Rotativa"),
    ("flexible", "Flexible"),
    ("tiempo-parcial", "Tiempo parcial"),
];
pub const CONTRACT_TYPES: &[(&str, &str)] = &[
    ("indefinido", "Indefinido"),
    ("plazo-fijo", "A plazo fijo"),
    ("servicios", "Por servicios (freelance)"),
    ("pasantia", "Pasantía"),
    ("por-obra", "Por obra determinada"),
];
pub const EMPLOYMENT_SECTORS: &[(&str, &str)] = &[
    ("technology", "Tecnología"),
    ("finance", "Finanzas y banca"),
    ("education", "Educación"),
    ("health", "Salud"),
    ("retail", "Comercio y ventas"),
    ("manufacturing", "Manufactura e industria"),
    ("agriculture", "Agricultura y agroindustria"),
    ("construction", "Construcción e inmobiliaria"),
    ("logistics", "Logística y transporte"),
    ("tourism", "Turismo, hotelería y restaurantes"),
    ("telecommunications", "Telecomunicaciones"),
    ("outsourcing", "Servicios empresariales y call center"),
    ("energy", "Energía y minería"),
    ("media", "Medios, publicidad y marketing"),
    ("government", "Sector público"),
    ("nonprofit", "Organizaciones sin fines de lucro"),
    ("legal", "Servicios legales y contables"),
    ("other", "Otro"),
];
/// Tipos de licencia de conducir de Guatemala.
pub const DRIVER_LICENSES: &[(&str, &str)] = &[
    ("a", "Licencia tipo A"),
    ("b", "Licencia tipo B"),
    ("c", "Licencia tipo C"),
    ("m", "Licencia tipo M"),
];
/// Planes de membresía de empresa. Solo el Empresarial (negocio.md); su `code` es el mismo del
/// plan en `plans`.
pub const MEMBERSHIP_TYPES: &[(&str, &str)] = &[("business", "Empresarial")];

/// Pasa un nombre a un code: minúsculas ASCII, sin tildes, con guiones entre palabras.
pub fn slug(texto: &str) -> String {
    let mut out = String::new();
    for c in texto.chars() {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' | 'Á' => 'a',
            'é' | 'è' | 'ë' | 'ê' | 'É' => 'e',
            'í' | 'ì' | 'ï' | 'î' | 'Í' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'Ó' => 'o',
            'ú' | 'ù' | 'ü' | 'û' | 'Ú' => 'u',
            'ñ' | 'Ñ' => 'n',
            c => c.to_ascii_lowercase(),
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// El code de un municipio: el del departamento más el del municipio, porque los nombres se
/// repiten entre departamentos (hay varios «San José» o «Concepción»).
pub fn code_municipio(depto: &str, municipio: &str) -> String {
    format!("{depto}-{}", slug(municipio))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ascii_slug(s: &str) -> bool {
        !s.is_empty()
            && s.split('-').all(|p| {
                !p.is_empty()
                    && p.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
    }

    #[test]
    fn slug_quita_tildes_y_signos() {
        assert_eq!(slug("Petén"), "peten");
        assert_eq!(slug("San José del Golfo"), "san-jose-del-golfo");
        assert_eq!(slug("Sacatepéquez"), "sacatepequez");
        assert_eq!(slug("Güipil  (Ñ)"), "guipil-n");
        assert_eq!(slug("Por servicios (freelance)"), "por-servicios-freelance");
    }

    /// Son 22 departamentos, sin repetir y con code ASCII.
    #[test]
    fn hay_22_departamentos_sin_repetir() {
        assert_eq!(GT_DEPARTAMENTOS.len(), 22);
        let mut codes = HashSet::new();
        let mut nombres = HashSet::new();
        for (code, nombre, _) in GT_DEPARTAMENTOS {
            assert!(ascii_slug(code), "code inválido: {code:?}");
            assert_eq!(*code, slug(nombre), "el code de {nombre} sale de su nombre");
            assert!(codes.insert(*code), "departamento repetido: {code}");
            assert!(nombres.insert(*nombre), "nombre repetido: {nombre}");
        }
        for esperado in [
            "guatemala",
            "quetzaltenango",
            "alta-verapaz",
            "peten",
            "zacapa",
            "jutiapa",
            "sacatepequez",
        ] {
            assert!(codes.contains(esperado), "falta el departamento {esperado}");
        }
    }

    /// Cada municipio cuelga de un departamento (la estructura lo obliga), ninguno está vacío ni
    /// repetido dentro de su departamento, y los codes derivados son únicos en todo el país.
    #[test]
    fn los_municipios_no_se_repiten_y_todos_tienen_departamento() {
        let mut codes = HashSet::new();
        let mut total = 0;
        for (depto, _, municipios) in GT_DEPARTAMENTOS {
            assert!(!municipios.is_empty(), "{depto} sin municipios");
            let mut nombres = HashSet::new();
            for m in *municipios {
                assert_eq!(m.trim(), *m, "espacios sobrantes en {m:?}");
                assert!(!m.is_empty());
                assert!(nombres.insert(*m), "{m} repetido en {depto}");
                let code = code_municipio(depto, m);
                assert!(ascii_slug(&code), "code inválido: {code:?}");
                assert!(codes.insert(code.clone()), "code repetido: {code}");
                total += 1;
            }
        }
        assert_eq!(total, 340, "Guatemala tiene 340 municipios");
    }

    /// El municipio de Guatemala (la capital) está en el departamento de Guatemala, y un nombre
    /// que se repite entre departamentos queda en cada uno.
    #[test]
    fn la_capital_y_los_nombres_repetidos_estan_donde_van() {
        let depto = |code: &str| {
            GT_DEPARTAMENTOS
                .iter()
                .find(|d| d.0 == code)
                .unwrap_or_else(|| panic!("falta {code}"))
        };
        assert!(depto("guatemala").2.contains(&"Guatemala"));
        assert_eq!(depto("guatemala").2.len(), 17);
        // «San José» existe en Escuintla y en Petén, y queda en ambos.
        assert!(depto("escuintla").2.contains(&"San José"));
        assert!(depto("peten").2.contains(&"San José"));
    }

    /// CT5: los rangos están ordenados, sin huecos ni solapes, y solo el último no tiene tope.
    #[test]
    fn los_rangos_salariales_no_se_solapan() {
        assert!(SALARY_RANGES.len() >= 5);
        let mut codes = HashSet::new();
        let mut siguiente_minimo = 0;
        for (i, (code, nombre, min, max)) in SALARY_RANGES.iter().enumerate() {
            assert!(ascii_slug(code) && codes.insert(*code), "code: {code}");
            assert!(!nombre.is_empty());
            assert_eq!(
                *min, siguiente_minimo,
                "{code}: hueco o solape con el anterior"
            );
            match max {
                Some(m) => {
                    assert!(m >= min, "{code}: máximo menor que el mínimo");
                    siguiente_minimo = m + 1;
                }
                None => assert_eq!(
                    i,
                    SALARY_RANGES.len() - 1,
                    "{code}: sin tope y no es el último"
                ),
            }
        }
    }

    /// Los catálogos simples: codes ASCII únicos, nombres no vacíos, y lo imprescindible.
    #[test]
    fn los_catalogos_simples_tienen_codes_validos_y_unicos() {
        for (nombre, lista, requeridos) in [
            (
                "working_shifts",
                WORKING_SHIFTS,
                &["diurna", "mixta", "nocturna"][..],
            ),
            (
                "contract_types",
                CONTRACT_TYPES,
                &["indefinido", "plazo-fijo", "servicios", "pasantia"][..],
            ),
            (
                "employment_sectors",
                EMPLOYMENT_SECTORS,
                &["technology", "finance", "education", "health", "retail"][..],
            ),
            (
                "driver_licenses",
                DRIVER_LICENSES,
                &["a", "b", "c", "m"][..],
            ),
            ("membership_types", MEMBERSHIP_TYPES, &["business"][..]),
        ] {
            let mut codes = HashSet::new();
            for (code, name) in lista {
                assert!(ascii_slug(code), "{nombre}: code inválido {code:?}");
                assert!(codes.insert(*code), "{nombre}: {code} repetido");
                assert!(!name.trim().is_empty());
            }
            for r in requeridos {
                assert!(codes.contains(r), "{nombre}: falta {r}");
            }
        }
    }
}
