//! Corpus SINTÉTICO de CVs con respuesta conocida, para MEDIR el extractor determinista.
//!
//! Atención: el equipo aún no tiene CVs reales. Estos CVs los escribimos nosotros, así que
//! miden regresiones y casos que imaginamos; NO demuestran calidad sobre CVs reales. Los
//! números de `medicion_del_corpus` son un termómetro interno, no una promesa de producto.
//!
//! La «respuesta correcta» es lo que una persona leyendo el CV diría que declara. Se escribió
//! antes de mirar qué hace el extractor. Correr con `-- --nocapture` para ver las métricas.

use std::collections::BTreeSet;
use std::io::Write;

use jobout_core_api::cv::extractor::{
    Confidence, CvExtractor, DeterministicExtractor, LanguageEntry, Proficiency, Proposal,
    SkillEntry,
};
use jobout_core_api::cv::text::{extract_text, CvKind, Limits};
use zip::write::SimpleFileOptions;

// ---------- catálogo (copia del sembrado de migration/src/seeds.rs) ----------

type SeedSkill = (&'static str, &'static str, &'static [&'static str]);

const SKILLS: &[SeedSkill] = &[
    ("javascript", "JavaScript", &["js", "ecmascript"]),
    ("typescript", "TypeScript", &["ts"]),
    ("python", "Python", &["py"]),
    ("java", "Java", &[]),
    ("kotlin", "Kotlin", &[]),
    ("swift", "Swift", &[]),
    ("csharp", "C#", &["c#", "c-sharp"]),
    ("cpp", "C++", &["c++"]),
    ("c", "C", &[]),
    ("go", "Go", &["golang"]),
    ("rust", "Rust", &[]),
    ("php", "PHP", &[]),
    ("ruby", "Ruby", &[]),
    ("scala", "Scala", &[]),
    ("dart", "Dart", &[]),
    ("elixir", "Elixir", &[]),
    ("bash", "Bash", &["shell", "sh"]),
    ("sql", "SQL", &[]),
    ("html", "HTML", &["html5"]),
    ("css", "CSS", &["css3"]),
    ("react", "React", &["reactjs", "react.js"]),
    ("nextjs", "Next.js", &["next", "next.js"]),
    ("vue", "Vue.js", &["vuejs", "vue.js"]),
    ("angular", "Angular", &["angularjs"]),
    ("svelte", "Svelte", &["sveltekit"]),
    ("nodejs", "Node.js", &["node", "node.js"]),
    ("express", "Express", &["expressjs"]),
    ("nestjs", "NestJS", &["nest"]),
    ("django", "Django", &[]),
    ("flask", "Flask", &[]),
    ("fastapi", "FastAPI", &[]),
    ("spring", "Spring Boot", &["spring-boot", "springboot"]),
    ("dotnet", ".NET", &[".net", "dotnet-core", "asp.net"]),
    ("laravel", "Laravel", &[]),
    ("rails", "Ruby on Rails", &["ror", "ruby-on-rails"]),
    ("flutter", "Flutter", &[]),
    ("reactnative", "React Native", &["react-native"]),
    ("android", "Android", &[]),
    ("ios", "iOS", &[]),
    ("tailwindcss", "Tailwind CSS", &["tailwind"]),
    ("graphql", "GraphQL", &[]),
    ("restapi", "REST API", &["rest", "restful", "api-rest"]),
    ("grpc", "gRPC", &[]),
    ("axum", "Axum", &[]),
    ("tokio", "Tokio", &[]),
    ("postgresql", "PostgreSQL", &["postgres", "psql"]),
    ("mysql", "MySQL", &[]),
    ("mariadb", "MariaDB", &[]),
    ("sqlserver", "SQL Server", &["mssql", "sql-server"]),
    ("oracle", "Oracle Database", &["oracle-db"]),
    ("sqlite", "SQLite", &[]),
    ("mongodb", "MongoDB", &["mongo"]),
    ("redis", "Redis", &[]),
    ("elasticsearch", "Elasticsearch", &["elastic"]),
    ("cassandra", "Cassandra", &[]),
    ("dynamodb", "DynamoDB", &[]),
    ("firebase", "Firebase", &[]),
    ("kafka", "Apache Kafka", &[]),
    ("rabbitmq", "RabbitMQ", &[]),
    ("aws", "Amazon Web Services", &["amazon-web-services"]),
    ("azure", "Microsoft Azure", &[]),
    ("gcp", "Google Cloud Platform", &["google-cloud"]),
    ("docker", "Docker", &[]),
    ("kubernetes", "Kubernetes", &["k8s", "kube"]),
    ("terraform", "Terraform", &[]),
    ("ansible", "Ansible", &[]),
    ("helm", "Helm", &[]),
    ("nginx", "Nginx", &[]),
    ("linux", "Linux", &[]),
    ("git", "Git", &[]),
    ("github", "GitHub", &[]),
    ("gitlab", "GitLab", &[]),
    ("githubactions", "GitHub Actions", &["github-actions"]),
    ("jenkins", "Jenkins", &[]),
    ("cicd", "CI/CD", &["ci-cd", "ci", "cd"]),
    ("prometheus", "Prometheus", &[]),
    ("grafana", "Grafana", &[]),
    ("jira", "Jira", &[]),
    ("figma", "Figma", &[]),
    ("postman", "Postman", &[]),
    ("webpack", "Webpack", &[]),
    ("vite", "Vite", &["vitejs"]),
    ("jest", "Jest", &[]),
    ("cypress", "Cypress", &[]),
    ("playwright", "Playwright", &[]),
    ("selenium", "Selenium", &[]),
    ("junit", "JUnit", &[]),
    ("pytest", "Pytest", &[]),
    ("pandas", "Pandas", &[]),
    ("numpy", "NumPy", &[]),
    ("scikitlearn", "scikit-learn", &["sklearn"]),
    ("tensorflow", "TensorFlow", &[]),
    ("pytorch", "PyTorch", &["torch"]),
    (
        "machinelearning",
        "Machine Learning",
        &["ml", "aprendizaje-automatico"],
    ),
    ("datascience", "Ciencia de datos", &["data-science"]),
    ("spark", "Apache Spark", &["pyspark"]),
    ("airflow", "Apache Airflow", &[]),
    ("powerbi", "Power BI", &["power-bi"]),
    ("tableau", "Tableau", &[]),
    ("excel", "Microsoft Excel", &["ms-excel"]),
    (
        "agile",
        "Metodologías ágiles",
        &["agil", "agile-methodologies"],
    ),
    ("scrum", "Scrum", &[]),
    ("kanban", "Kanban", &[]),
    ("tdd", "TDD", &["test-driven-development"]),
    ("microservices", "Microservicios", &["micro-services"]),
    (
        "oop",
        "Programación orientada a objetos",
        &["poo", "object-oriented"],
    ),
    (
        "designpatterns",
        "Patrones de diseño",
        &["design-patterns", "patrones-de-diseno"],
    ),
    ("websockets", "WebSockets", &["websocket"]),
    ("oauth", "OAuth", &["oauth2"]),
    ("jwt", "JWT", &["json-web-token"]),
    ("sass", "Sass", &["scss"]),
    ("bootstrap", "Bootstrap", &[]),
    ("wordpress", "WordPress", &[]),
    ("unity", "Unity", &["unity3d"]),
];

fn extractor() -> DeterministicExtractor {
    let skills = SKILLS
        .iter()
        .map(|(code, name, aliases)| SkillEntry {
            id: (*code).into(),
            code: (*code).into(),
            name: (*name).into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
        })
        .collect();
    let languages = [("es", "Español"), ("en", "Inglés"), ("pt", "Portugués")]
        .iter()
        .map(|(code, name)| LanguageEntry {
            id: (*code).into(),
            code: (*code).into(),
            name: (*name).into(),
        })
        .collect();
    DeterministicExtractor::new(skills, languages)
}

// ---------- generadores de archivos (copiados de tests/cv_text.rs, con varias líneas) ----------

/// PDF mínimo y válido; una línea de texto por entrada de `lines` (solo ASCII).
/// `one_block`: todas las líneas dentro de un único `BT … ET` (como hacen algunos
/// generadores); si no, cada línea es su propio bloque de texto (como hace Word).
fn pdf_lines(lines: &[&str], one_block: bool) -> Vec<u8> {
    let mut stream = String::new();
    if one_block {
        stream.push_str("BT /F1 11 Tf 40 760 Td\n");
    }
    for (i, l) in lines.iter().enumerate() {
        let esc = l
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        if one_block {
            stream.push_str(&format!("({esc}) Tj 0 -14 Td\n"));
        } else {
            let y = 760 - 14 * i;
            stream.push_str(&format!("BT /F1 11 Tf 40 {y} Td ({esc}) Tj ET\n"));
        }
    }
    if one_block {
        stream.push_str("ET");
    }
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>"
            .into(),
        format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            stream.len(),
            stream
        ),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objs.len() + 1,
            xref
        )
        .as_bytes(),
    );
    out
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn docx(paragraphs: &[&str]) -> Vec<u8> {
    let body: String = paragraphs
        .iter()
        .map(|p| {
            format!(
                "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                escape_xml(p)
            )
        })
        .collect();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:body>{body}</w:body></w:document>"
    );
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [
        ("[Content_Types].xml", b"<Types/>".to_vec()),
        ("word/document.xml", xml.into_bytes()),
    ] {
        w.start_file(name, opts).unwrap();
        w.write_all(&data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

// ---------- el corpus ----------

#[derive(Clone, Copy)]
enum Format {
    Text,
    Pdf,
    /// PDF con todas las líneas en un solo bloque de texto.
    PdfOneBlock,
    Docx,
}

struct Cv {
    name: &'static str,
    format: Format,
    /// Líneas (o párrafos, o líneas del PDF) del CV.
    lines: &'static [&'static str],
    /// `code` de las skills que el CV declara.
    skills: &'static [&'static str],
    /// (código de idioma, nivel si el CV lo dice).
    languages: &'static [(&'static str, Option<Proficiency>)],
    /// (empresa, cargo, fecha de inicio `YYYY-MM-DD`).
    experiences: &'static [(&'static str, &'static str, &'static str)],
    /// Fragmentos que JAMÁS pueden aparecer en la propuesta serializada.
    forbidden: &'static [&'static str],
}

use Proficiency::{Advanced, Basic, Intermediate, Native};

const CORPUS: &[Cv] = &[
    Cv {
        name: "01 es formal con secciones y guion largo",
        format: Format::Text,
        lines: &[
            "María López",
            "Desarrolladora de Software",
            "",
            "Perfil",
            "Desarrolladora backend con 6 años de experiencia en APIs y datos.",
            "",
            "Experiencia laboral",
            "Desarrolladora Backend — Banco Ejemplo (2019–2022)",
            "- Servicios en Python y Django con PostgreSQL",
            "- Despliegue con Docker en AWS",
            "Ingeniera de Software — Tienda Online SA (2022–presente)",
            "- Migración a microservicios con Kafka y Redis",
            "",
            "Educación",
            "Universidad Nacional — Ingeniería en Sistemas (2013–2018)",
            "",
            "Idiomas",
            "Español: nativo",
            "Inglés: avanzado",
        ],
        skills: &[
            "python",
            "django",
            "postgresql",
            "docker",
            "aws",
            "microservices",
            "kafka",
            "redis",
        ],
        languages: &[("es", Some(Native)), ("en", Some(Advanced))],
        experiences: &[
            ("Banco Ejemplo", "Desarrolladora Backend", "2019-01-01"),
            ("Tienda Online SA", "Ingeniera de Software", "2022-01-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "02 en pipes, fechas Jan 2020 - Present",
        format: Format::Text,
        lines: &[
            "John Smith",
            "Senior Software Engineer",
            "",
            "Summary",
            "Full-stack engineer who ships quickly and likes clean code.",
            "",
            "Work Experience",
            "Globex Corp | Senior Engineer | Jan 2020 – Present",
            "* Built REST APIs in TypeScript and Node.js",
            "* Deployed on Kubernetes with Terraform and GitHub Actions",
            "Initech | Software Developer | Mar 2016 – Dec 2019",
            "* Maintained a Java and Spring Boot monolith, MySQL database",
            "",
            "Skills",
            "React, TypeScript, Node.js, Kubernetes, Terraform, Java",
            "",
            "Languages",
            "English (native), Spanish (B2)",
        ],
        skills: &[
            "restapi",
            "typescript",
            "nodejs",
            "kubernetes",
            "terraform",
            "githubactions",
            "java",
            "spring",
            "mysql",
            "react",
        ],
        languages: &[("en", Some(Native)), ("es", Some(Intermediate))],
        experiences: &[
            ("Globex Corp", "Senior Engineer", "2020-01-01"),
            ("Initech", "Software Developer", "2016-03-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "03 es sin encabezados, prosa",
        format: Format::Text,
        lines: &[
            "Carlos Méndez - Desarrollador Full Stack",
            "Soy desarrollador full stack, trabajo con React, Node y MongoDB desde hace años.",
            "Trabajé en Startup Uno como Desarrollador Frontend de 2018 a 2020.",
            "Luego en Consultora Dos como Líder Técnico entre 2020 y 2023.",
            "Hablo inglés a nivel intermedio.",
        ],
        skills: &["react", "nodejs", "mongodb"],
        languages: &[("en", Some(Intermediate))],
        experiences: &[
            ("Startup Uno", "Desarrollador Frontend", "2018-01-01"),
            ("Consultora Dos", "Líder Técnico", "2020-01-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "04 en 'at', viñetas y línea Stack",
        format: Format::Text,
        lines: &[
            "Priya Nair",
            "Backend Developer",
            "",
            "Experience",
            "Backend Developer at Fintrack (2021 - 2024)",
            "• Designed payment services in Go and Rust",
            "• Stack: Go, Rust, PostgreSQL, Redis, gRPC, Docker",
            "QA Engineer at Retailio (Feb 2018 - Aug 2021)",
            "• Automated tests with Selenium and Pytest. Stack: Python, Jenkins",
            "",
            "Education",
            "BSc Computer Science, University of Pune, 2014 - 2018",
        ],
        skills: &[
            "go",
            "rust",
            "postgresql",
            "redis",
            "grpc",
            "docker",
            "selenium",
            "pytest",
            "python",
            "jenkins",
        ],
        languages: &[],
        experiences: &[
            ("Fintrack", "Backend Developer", "2021-01-01"),
            ("Retailio", "QA Engineer", "2018-02-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "05 es 'Tecnologías:', fechas 03/2019",
        format: Format::Text,
        lines: &[
            "Luis Quispe",
            "Ingeniero DevOps",
            "",
            "Experiencia",
            "Ingeniero DevOps | Nube Andina | 03/2019 – 06/2021",
            "Tecnologías: Terraform, Ansible, Helm, K8s, CI/CD, Linux",
            "Practicante de Sistemas | Telecom Sur | 06/2018 – 02/2019",
            "Tecnologías: Bash, Git, Nginx",
            "",
            "Idiomas",
            "Inglés: avanzado",
            "Portugués: básico",
        ],
        skills: &[
            "terraform",
            "ansible",
            "helm",
            "kubernetes",
            "cicd",
            "linux",
            "bash",
            "git",
            "nginx",
        ],
        languages: &[("en", Some(Advanced)), ("pt", Some(Basic))],
        experiences: &[
            ("Nube Andina", "Ingeniero DevOps", "2019-03-01"),
            ("Telecom Sur", "Practicante de Sistemas", "2018-06-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "06 alias y mayúsculas raras, sin experiencia",
        format: Format::Text,
        lines: &[
            "Developer profile",
            "Skilled in NodeJS, postgres, K8s, CI/CD, ReactJS, Golang, TS, py and Mongo.",
            "Also use vuejs, springboot, sklearn, PyTorch and SCSS.",
        ],
        skills: &[
            "nodejs",
            "postgresql",
            "kubernetes",
            "cicd",
            "react",
            "go",
            "typescript",
            "python",
            "mongodb",
            "vue",
            "spring",
            "scikitlearn",
            "pytorch",
            "sass",
        ],
        languages: &[],
        experiences: &[],
        forbidden: &[],
    },
    Cv {
        name: "07 es idiomas con niveles MCER",
        format: Format::Text,
        lines: &[
            "Idiomas",
            "- Inglés (C1)",
            "- Español (nativo)",
            "- Portugués (A2)",
        ],
        skills: &[],
        languages: &[
            ("en", Some(Advanced)),
            ("es", Some(Native)),
            ("pt", Some(Basic)),
        ],
        experiences: &[],
        forbidden: &[],
    },
    Cv {
        name: "08 en idiomas en prosa",
        format: Format::Text,
        lines: &[
            "About me",
            "I am fluent in English and basic Portuguese; Spanish is my native language.",
        ],
        skills: &[],
        languages: &[
            ("en", Some(Advanced)),
            ("pt", Some(Basic)),
            ("es", Some(Native)),
        ],
        experiences: &[],
        forbidden: &[],
    },
    Cv {
        name: "09 es experiencias en dos y tres líneas (empresa/cargo/fecha)",
        format: Format::Text,
        lines: &[
            "EXPERIENCIA PROFESIONAL",
            "Consultora Delta",
            "Analista de Datos",
            "2020 – 2023",
            "- Dashboards en Power BI y Tableau, consultas SQL",
            "Hospital Central",
            "Desarrollador de Sistemas",
            "Septiembre 2016 – Diciembre 2019",
            "- Aplicaciones en C# y .NET con SQL Server y Excel",
        ],
        skills: &[
            "powerbi", "tableau", "sql", "csharp", "dotnet", "sqlserver", "excel",
        ],
        languages: &[],
        experiences: &[
            ("Consultora Delta", "Analista de Datos", "2020-01-01"),
            ("Hospital Central", "Desarrollador de Sistemas", "2016-09-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "10 en cargo, empresa y fecha en tres líneas",
        format: Format::Text,
        lines: &[
            "Work History",
            "Product Engineer",
            "Lumen Labs",
            "Jan 2022 – Present",
            "- React, Next.js and GraphQL front-ends",
            "Junior Developer",
            "Pixel Studio",
            "2019 – 2021",
            "- WordPress and PHP sites, Sass",
        ],
        skills: &["react", "nextjs", "graphql", "wordpress", "php", "sass"],
        languages: &[],
        experiences: &[
            ("Lumen Labs", "Product Engineer", "2022-01-01"),
            ("Pixel Studio", "Junior Developer", "2019-01-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "11 es con ruido: educación, cursos, referencias, redes, intereses",
        format: Format::Text,
        lines: &[
            "Ana Torres",
            "ana.torres@mail.com | +34 600 111 222 | linkedin.com/in/anatorres | github.com/anat",
            "Calle Falsa 123, Madrid",
            "",
            "Experiencia",
            "Desarrolladora Mobile — Appito (2020 – 2023)",
            "Apps en Flutter y Dart con Firebase",
            "",
            "Formación",
            "Grado en Informática, Universidad de Madrid (2014 – 2018)",
            "Curso de Liderazgo en Coursera (2019)",
            "",
            "Referencias",
            "Pedro Gómez, Gerente en Appito, disponible a solicitud",
            "",
            "Intereses",
            "Ajedrez, senderismo, Go (el juego de mesa)",
        ],
        skills: &["flutter", "dart", "firebase"],
        languages: &[],
        experiences: &[("Appito", "Desarrolladora Mobile", "2020-01-01")],
        forbidden: &[
            "600 111 222",
            "anatorres",
            "mail.com",
            "Calle Falsa",
            "github.com",
        ],
    },
    Cv {
        name: "12 PDF de dos columnas intercaladas",
        format: Format::Pdf,
        lines: &[
            "CONTACT                      EXPERIENCE",
            "ana@x.com                    Backend Developer, Orbit (2018 - 2021)",
            "SKILLS                       Built APIs in Python and Flask",
            "Python, Flask                Led migration to PostgreSQL",
            "Docker, Linux                Data Engineer, Nimbus (2021 - Present)",
            "LANGUAGES                    Airflow pipelines on GCP",
            "English - advanced           Spark jobs",
        ],
        skills: &[
            "python",
            "flask",
            "docker",
            "linux",
            "postgresql",
            "airflow",
            "gcp",
            "spark",
        ],
        languages: &[("en", Some(Advanced))],
        experiences: &[
            ("Orbit", "Backend Developer", "2018-01-01"),
            ("Nimbus", "Data Engineer", "2021-01-01"),
        ],
        forbidden: &["ana@x.com"],
    },
    Cv {
        name: "13 DOCX de tabla: celdas separadas por tabulación",
        format: Format::Docx,
        lines: &[
            "Skill\tLevel\tYears",
            "Java\tExpert\t8",
            "Spring Boot\tAdvanced\t5",
            "SQL\tAdvanced\t7",
            "Language\tLevel",
            "English\tAdvanced",
            "Spanish\tNative",
            "Company\tRole\tFrom\tTo",
            "Zeta Bank\tJava Developer\t2016\t2020",
            "Omega Retail\tTech Lead\t2020\tPresent",
        ],
        skills: &["java", "spring", "sql"],
        languages: &[("en", Some(Advanced)), ("es", Some(Native))],
        experiences: &[
            ("Zeta Bank", "Java Developer", "2016-01-01"),
            ("Omega Retail", "Tech Lead", "2020-01-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "14 en mínimo de 5 líneas",
        format: Format::Text,
        lines: &[
            "Sam Lee",
            "Frontend developer",
            "Skills: HTML, CSS, JavaScript",
            "Languages: English",
            "Worked at Pixelworks 2022-2024",
        ],
        skills: &["html", "css", "javascript"],
        languages: &[("en", None)],
        experiences: &[],
        forbidden: &[],
    },
    Cv {
        name: "15 es mínimo de 5 líneas con una experiencia",
        format: Format::Text,
        lines: &[
            "Jorge Ruiz",
            "Programador",
            "Rust, Docker, Git",
            "Inglés básico",
            "Acme Corp — Programador (2023–presente)",
        ],
        skills: &["rust", "docker", "git"],
        languages: &[("en", Some(Basic))],
        experiences: &[("Acme Corp", "Programador", "2023-01-01")],
        forbidden: &[],
    },
    Cv {
        name: "16 es con todos los datos de contacto",
        format: Format::Text,
        lines: &[
            "CURRICULUM VITAE",
            "Nombre: Elena Ruiz",
            "Teléfono: +502 5555 1234",
            "Móvil: (502) 5555-1234",
            "Email: elena.ruiz@correo.gt",
            "LinkedIn: https://www.linkedin.com/in/elenaruiz",
            "Web: elena-dev.gt/portafolio",
            "Dirección: 5a Avenida 12-34, Zona 10, Ciudad de Guatemala",
            "WhatsApp wa.me/50255551234",
            "Experiencia",
            "Ingeniera de Software — Banco Industrial (jun 2020 – actual)",
            "- Java, Spring Boot y MySQL",
            "Idiomas: Español nativo, Inglés intermedio",
        ],
        skills: &["java", "spring", "mysql"],
        languages: &[("es", Some(Native)), ("en", Some(Intermediate))],
        experiences: &[(
            "Banco Industrial",
            "Ingeniera de Software",
            "2020-06-01",
        )],
        forbidden: &[
            "5555",
            "elena.ruiz",
            "linkedin",
            "correo.gt",
            "Zona 10",
            "50255551234",
            "portafolio",
            "Avenida",
            "wa.me",
        ],
    },
    Cv {
        name: "17 en con palabras comunes que son alias (go, next, rest, nest, shell)",
        format: Format::Text,
        lines: &[
            "Professional Summary",
            "Team player who likes to go the extra mile. Next steps were planned with the rest of the team; I helped them nest ideas and shell out less.",
            "",
            "Experience",
            "Support Analyst, Helpdesk Inc (2017 - 2019)",
            "- Resolved tickets in Jira and Zendesk",
            "",
            "Skills",
            "Python",
        ],
        skills: &["jira", "python"],
        languages: &[],
        experiences: &[("Helpdesk Inc", "Support Analyst", "2017-01-01")],
        forbidden: &[],
    },
    Cv {
        name: "18 DOCX es con tildes y mes en palabras",
        format: Format::Docx,
        lines: &[
            "Sofía Ramírez",
            "Perfil",
            "Ingeniera de datos con enfoque en calidad y automatización.",
            "Experiencia",
            "Ingeniera de Datos | Cielo Analytics | Mayo 2019 – Actualidad",
            "- Pipelines con Airflow y Spark; modelos con scikit-learn y pandas",
            "Analista Junior | Mercado Sur | 2017 – 2019",
            "- Reportes en Excel y SQL",
            "Idiomas",
            "Español (nativo)",
            "Inglés (avanzado)",
        ],
        skills: &["airflow", "spark", "scikitlearn", "pandas", "excel", "sql"],
        languages: &[("es", Some(Native)), ("en", Some(Advanced))],
        experiences: &[
            ("Cielo Analytics", "Ingeniera de Datos", "2019-05-01"),
            ("Mercado Sur", "Analista Junior", "2017-01-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "19 DOCX en con comas, GitHub Actions y React Native",
        format: Format::Docx,
        lines: &[
            "Daniel Okafor",
            "Mobile Developer",
            "Experience",
            "Mobile Developer, Zenith Apps, Jun 2021 – Present",
            "- Swift and Kotlin apps, Firebase, CI/CD with Jenkins",
            "Android Developer, Kora, 2018 - 2021",
            "- Java Android apps, REST APIs, React Native prototype",
            "Languages",
            "English: native",
            "Spanish: intermediate",
        ],
        skills: &[
            "swift",
            "kotlin",
            "firebase",
            "cicd",
            "jenkins",
            "java",
            "android",
            "restapi",
            "reactnative",
        ],
        languages: &[("en", Some(Native)), ("es", Some(Intermediate))],
        experiences: &[
            ("Zenith Apps", "Mobile Developer", "2021-06-01"),
            ("Kora", "Android Developer", "2018-01-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "20 PDF en limpio con guion como separador",
        format: Format::Pdf,
        lines: &[
            "Ravi Patel",
            "Data Scientist",
            "Experience",
            "Data Scientist - Quantix (2020 - Present)",
            "- Models in Python with TensorFlow and PyTorch",
            "Analyst - Bluebank (2017 - 2020)",
            "- Dashboards in Tableau",
            "Languages",
            "English (C2)",
        ],
        skills: &["python", "tensorflow", "pytorch", "tableau"],
        languages: &[("en", Some(Advanced))],
        experiences: &[
            ("Quantix", "Data Scientist", "2020-01-01"),
            ("Bluebank", "Analyst", "2017-01-01"),
        ],
        forbidden: &[],
    },
    Cv {
        name: "21 PDF es (ASCII) con 'en' como separador",
        format: Format::Pdf,
        lines: &[
            "Luisa Fernandez",
            "Desarrolladora Full Stack",
            "Experiencia",
            "Desarrolladora Full Stack en Kappa Digital (ene 2021 - dic 2023)",
            "- Angular, NestJS, MySQL y Docker",
            "Idiomas",
            "Ingles avanzado, Espanol nativo",
        ],
        skills: &["angular", "nestjs", "mysql", "docker"],
        languages: &[("en", Some(Advanced)), ("es", Some(Native))],
        experiences: &[(
            "Kappa Digital",
            "Desarrolladora Full Stack",
            "2021-01-01",
        )],
        forbidden: &[],
    },
    Cv {
        name: "22 PDF con todas las líneas en un solo bloque de texto",
        format: Format::PdfOneBlock,
        lines: &[
            "Mia Chen",
            "Site Reliability Engineer",
            "Experience",
            "Site Reliability Engineer, Cloudly (Mar 2020 - Present)",
            "- Prometheus and Grafana on AWS with Terraform",
            "Languages",
            "English (native)",
            "Spanish (B1)",
        ],
        skills: &["prometheus", "grafana", "aws", "terraform"],
        languages: &[("en", Some(Native)), ("es", Some(Intermediate))],
        experiences: &[("Cloudly", "Site Reliability Engineer", "2020-03-01")],
        forbidden: &[],
    },
];

// ---------- obtención del texto y medición ----------

fn text_of(cv: &Cv) -> String {
    let limits = Limits::default();
    match cv.format {
        Format::Text => cv.lines.join("\n"),
        Format::Pdf | Format::PdfOneBlock => {
            let bytes = pdf_lines(cv.lines, matches!(cv.format, Format::PdfOneBlock));
            extract_text(&bytes, CvKind::Pdf, &limits)
                .unwrap_or_else(|e| panic!("{}: PDF ilegible: {e:?}", cv.name))
                .text
        }
        Format::Docx => {
            extract_text(&docx(cv.lines), CvKind::Docx, &limits)
                .unwrap_or_else(|e| panic!("{}: DOCX ilegible: {e:?}", cv.name))
                .text
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Counts {
    tp: usize,
    fp: usize,
    fn_: usize,
}

impl Counts {
    fn precision(&self) -> f64 {
        if self.tp + self.fp == 0 {
            1.0
        } else {
            self.tp as f64 / (self.tp + self.fp) as f64
        }
    }
    fn recall(&self) -> f64 {
        if self.tp + self.fn_ == 0 {
            1.0
        } else {
            self.tp as f64 / (self.tp + self.fn_) as f64
        }
    }
    fn add(&mut self, other: Counts) {
        self.tp += other.tp;
        self.fp += other.fp;
        self.fn_ += other.fn_;
    }
}

fn compare(
    expected: &BTreeSet<String>,
    got: &BTreeSet<String>,
) -> (Counts, Vec<String>, Vec<String>) {
    let missing: Vec<String> = expected.difference(got).cloned().collect();
    let extra: Vec<String> = got.difference(expected).cloned().collect();
    (
        Counts {
            tp: expected.intersection(got).count(),
            fp: extra.len(),
            fn_: missing.len(),
        },
        missing,
        extra,
    )
}

fn exp_key(company: &str, title: &str, start: &str) -> String {
    format!(
        "{} | {} | {}",
        company.to_lowercase(),
        title.to_lowercase(),
        start
    )
}

#[derive(Default, Clone, Copy)]
struct Totals {
    skills: Counts,
    languages: Counts,
    language_levels: Counts,
    experiences: Counts,
    /// Experiencias propuestas con confianza `media` o `alta`.
    experiences_firm: Counts,
}

fn evaluate(cv: &Cv, p: &Proposal, verbose: bool) -> Totals {
    let mut t = Totals::default();
    let set = |v: Vec<String>| v.into_iter().collect::<BTreeSet<_>>();

    let (c, miss, extra) = compare(
        &set(cv.skills.iter().map(|s| s.to_string()).collect()),
        &set(p.skills.iter().map(|s| s.code.clone()).collect()),
    );
    t.skills = c;
    report(verbose, cv, "skills", &miss, &extra);

    let (c, miss, extra) = compare(
        &set(cv.languages.iter().map(|(l, _)| l.to_string()).collect()),
        &set(p.languages.iter().map(|l| l.code.clone()).collect()),
    );
    t.languages = c;
    report(verbose, cv, "idiomas", &miss, &extra);

    // Idioma + nivel: solo cuenta como acierto si el nivel coincide (o el CV no dice nivel).
    let level_of = |l: &Option<Proficiency>| l.map(|x| format!("{x:?}")).unwrap_or_default();
    let expected_lv = set(cv
        .languages
        .iter()
        .map(|(c, l)| format!("{c}:{}", level_of(l)))
        .collect());
    let got_lv = set(p
        .languages
        .iter()
        .map(|l| {
            let known = cv.languages.iter().find(|(c, _)| *c == l.code);
            let lv = match known {
                Some((_, None)) => String::new(),
                _ => format!("{:?}", l.proficiency),
            };
            format!("{}:{lv}", l.code)
        })
        .collect());
    let (c, miss, extra) = compare(&expected_lv, &got_lv);
    t.language_levels = c;
    report(verbose, cv, "idiomas+nivel", &miss, &extra);

    let expected_exp = set(cv
        .experiences
        .iter()
        .map(|(c, ti, s)| exp_key(c, ti, s))
        .collect());
    let got_exp = set(p
        .experiences
        .iter()
        .map(|e| exp_key(&e.company, &e.title, &e.start_date))
        .collect());
    let (c, miss, extra) = compare(&expected_exp, &got_exp);
    t.experiences = c;
    report(verbose, cv, "experiencias", &miss, &extra);

    let got_firm = set(p
        .experiences
        .iter()
        .filter(|e| e.confidence >= Confidence::Medium)
        .map(|e| exp_key(&e.company, &e.title, &e.start_date))
        .collect());
    t.experiences_firm = compare(&expected_exp, &got_firm).0;
    t
}

fn report(verbose: bool, cv: &Cv, what: &str, missing: &[String], extra: &[String]) {
    if verbose && (!missing.is_empty() || !extra.is_empty()) {
        println!("  [{}] {what}", cv.name);
        for m in missing {
            println!("      falta : {m}");
        }
        for e in extra {
            println!("      sobra : {e}");
        }
    }
}

fn measure(verbose: bool) -> Totals {
    let ex = extractor();
    let mut all = Totals::default();
    for cv in CORPUS {
        let p = ex.extract(&text_of(cv));
        let t = evaluate(cv, &p, verbose);
        all.skills.add(t.skills);
        all.languages.add(t.languages);
        all.language_levels.add(t.language_levels);
        all.experiences.add(t.experiences);
        all.experiences_firm.add(t.experiences_firm);
    }
    all
}

fn print_row(name: &str, c: &Counts) {
    println!(
        "{name:<34} precisión {:>5.1}%   recall {:>5.1}%   (aciertos {}, sobran {}, faltan {})",
        c.precision() * 100.0,
        c.recall() * 100.0,
        c.tp,
        c.fp,
        c.fn_
    );
}

/// Umbrales: lo que el extractor cumple HOY, para que no retroceda. NO son el objetivo.
/// Lo deseable (≥ 90 % en todo) todavía no se cumple en las categorías marcadas «bajo lo deseable».
#[test]
fn medicion_del_corpus() {
    let t = measure(true);
    println!("\n==== Corpus sintético ({} CVs) ====", CORPUS.len());
    print_row("skills", &t.skills);
    print_row("idiomas (código)", &t.languages);
    print_row("idiomas (código + nivel)", &t.language_levels);
    print_row("experiencias (empresa+cargo+inicio)", &t.experiences);
    print_row("experiencias (solo conf. ≥ media)", &t.experiences_firm);

    for (name, c, min_p, min_r) in THRESHOLDS.iter().map(|(n, k, p, r)| {
        let c = match *k {
            0 => t.skills,
            1 => t.languages,
            2 => t.language_levels,
            3 => t.experiences,
            _ => t.experiences_firm,
        };
        (*n, c, *p, *r)
    }) {
        assert!(
            c.precision() >= min_p,
            "{name}: precisión {:.3} < umbral {min_p}",
            c.precision()
        );
        assert!(
            c.recall() >= min_r,
            "{name}: recall {:.3} < umbral {min_r}",
            c.recall()
        );
    }
}

// (nombre, categoría, precisión mínima, recall mínimo)
// Umbrales = lo que el extractor cumple HOY sobre el corpus (no retroceder). Antes de jb-32:
// skills 93.7/99.2, idiomas+nivel 92.9/89.7, experiencias 85.2/76.7 (precisión/recall, %).
// Por debajo de lo deseable (≥ 95 %): experiencias, por CVs de columnas intercaladas (12),
// prosa sin encabezados (03) y líneas sin cargo (14). Skills e idiomas rozan el 100 % porque el
// corpus lo escribimos nosotros con el extractor a la vista: es un techo optimista, no real.
const THRESHOLDS: &[(&str, u8, f64, f64)] = &[
    ("skills", 0, 0.99, 0.99),
    ("idiomas (código)", 1, 0.99, 0.99),
    ("idiomas (código + nivel)", 2, 0.99, 0.99),
    ("experiencias", 3, 0.89, 0.89),
    ("experiencias conf. ≥ media", 4, 0.95, 0.89),
];

/// D8 sobre todo el corpus: ni una cadena prohibida en el JSON de ninguna propuesta,
/// incluso pasando por PDF y DOCX.
#[test]
fn d8_el_corpus_jamas_filtra_contacto() {
    let ex = extractor();
    let mut con_prohibidos = 0;
    for cv in CORPUS {
        let json = serde_json::to_string(&ex.extract(&text_of(cv))).unwrap();
        for malo in cv.forbidden {
            con_prohibidos += 1;
            assert!(
                !json.to_lowercase().contains(&malo.to_lowercase()),
                "{}: el JSON contiene {malo:?}: {json}",
                cv.name
            );
        }
        for generico in ["@", "http", "www.", "linkedin", "github.com"] {
            assert!(
                !json.to_lowercase().contains(generico),
                "{}: el JSON contiene {generico:?}: {json}",
                cv.name
            );
        }
    }
    assert!(
        con_prohibidos >= 10,
        "el corpus perdió sus casos de contacto"
    );
}

#[test]
fn el_corpus_tiene_variedad() {
    assert!(CORPUS.len() >= 14);
    let n = |f: fn(&Format) -> bool| CORPUS.iter().filter(|c| f(&c.format)).count();
    assert!(n(|f| matches!(f, Format::Pdf)) >= 3);
    assert!(n(|f| matches!(f, Format::Docx)) >= 3);
}

#[test]
fn el_extractor_es_determinista_sobre_el_corpus() {
    let ex = extractor();
    for cv in CORPUS {
        let t = text_of(cv);
        assert_eq!(ex.extract(&t), ex.extract(&t), "{}", cv.name);
    }
}

// ---------- casos puntuales: cada uno fue rojo antes de su mejora ----------

fn cv_de(prefijo: &str) -> &'static Cv {
    CORPUS
        .iter()
        .find(|c| c.name.starts_with(prefijo))
        .unwrap_or_else(|| panic!("no hay CV {prefijo}"))
}

fn proponer(cv: &Cv) -> Proposal {
    extractor().extract(&text_of(cv))
}

/// Falla (con el detalle) si la categoría tiene sobrantes o faltantes en ese CV.
fn assert_exacto(prefijo: &str, skills: bool, languages: bool, experiences: bool) {
    let cv = cv_de(prefijo);
    let t = evaluate(cv, &proponer(cv), true);
    let limpio = |c: Counts| c.fp == 0 && c.fn_ == 0;
    if skills {
        assert!(limpio(t.skills), "{}: skills", cv.name);
    }
    if languages {
        assert!(
            limpio(t.languages) && limpio(t.language_levels),
            "{}: idiomas",
            cv.name
        );
    }
    if experiences {
        assert!(limpio(t.experiences), "{}: experiencias", cv.name);
    }
}

#[test]
fn mejora_una_skill_dentro_de_otra_mas_larga_no_cuenta_aparte() {
    // «GitHub Actions» no es GitHub; «React Native» no es React.
    assert_exacto("02", true, false, false);
    assert_exacto("19", true, false, false);
}

#[test]
fn mejora_palabras_comunes_no_son_skills_en_prosa() {
    // go / next / rest / nest / shell en una frase; «Go» como afición.
    assert_exacto("17", true, false, false);
    assert_exacto("11", true, false, false);
}

#[test]
fn mejora_analytics_no_es_un_cargo() {
    assert_exacto("18", false, false, true);
}

#[test]
fn mejora_pdf_con_todo_en_un_solo_bloque_de_texto() {
    assert_exacto("22", true, true, true);
}

#[test]
fn mejora_el_nivel_antes_del_idioma_es_del_idioma_siguiente() {
    // «fluent in English and basic Portuguese; Spanish is my native language»
    assert_exacto("08", false, true, false);
}

#[test]
fn mejora_tabla_con_celdas_separadas_por_tabulacion() {
    assert_exacto("13", true, true, true);
}

#[test]
fn mejora_lo_que_no_se_entiende_se_propone_con_confianza_baja() {
    for prefijo in ["03", "14"] {
        let cv = cv_de(prefijo);
        let verdad: BTreeSet<String> = cv
            .experiences
            .iter()
            .map(|(c, t, s)| exp_key(c, t, s))
            .collect();
        for e in proponer(cv).experiences {
            if !verdad.contains(&exp_key(&e.company, &e.title, &e.start_date)) {
                assert_eq!(e.confidence, Confidence::Low, "{}: {e:?}", cv.name);
            }
        }
    }
}

#[test]
fn una_skill_de_palabra_comun_nunca_es_de_confianza_alta() {
    // «Swift» (lenguaje) también es un adjetivo: aunque se acepte, queda en `media`.
    let p = proponer(cv_de("19"));
    let swift = p.skills.iter().find(|s| s.code == "swift").expect("Swift");
    assert_eq!(swift.confidence, Confidence::Medium);
}
