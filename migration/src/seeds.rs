//! Datos semilla de los catálogos. Los `code` de skills son minúsculas ASCII sin espacios ni
//! tildes; los nombres y alias son lo que la gente escribe en un CV.

/// Skills de software: (code, nombre, alias). Catálogo cerrado y curado a mano.
pub const SKILLS: &[(&str, &str, &[&str])] = &[
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

/// Idiomas (ISO 639-1): (code, nombre en español).
pub const LANGUAGES: &[(&str, &str)] = &[("es", "Español"), ("en", "Inglés"), ("pt", "Portugués")];

/// Niveles académicos, de menor a mayor.
pub const ACADEMIC_LEVELS: &[(&str, &str)] = &[
    ("secondary", "Diversificado o bachillerato"),
    ("technical", "Técnico"),
    ("bachelor", "Licenciatura"),
    ("postgraduate", "Postgrado"),
    ("master", "Maestría"),
    ("doctorate", "Doctorado"),
];

/// Profesiones de software.
pub const PROFESSIONS: &[(&str, &str)] = &[
    ("software-engineer", "Ingeniero de software"),
    ("frontend-developer", "Desarrollador frontend"),
    ("backend-developer", "Desarrollador backend"),
    ("fullstack-developer", "Desarrollador full stack"),
    ("mobile-developer", "Desarrollador móvil"),
    ("devops-engineer", "Ingeniero DevOps"),
    ("data-engineer", "Ingeniero de datos"),
    ("data-scientist", "Científico de datos"),
    ("qa-engineer", "Ingeniero de calidad (QA)"),
    ("ux-ui-designer", "Diseñador UX/UI"),
    ("product-manager", "Gerente de producto"),
    ("systems-analyst", "Analista de sistemas"),
];

/// Jerarquías laborales, de menor a mayor.
pub const JOB_HIERARCHIES: &[(&str, &str)] = &[
    ("intern", "Practicante"),
    ("junior", "Junior"),
    ("semi-senior", "Semi senior"),
    ("senior", "Senior"),
    ("lead", "Líder técnico"),
    ("manager", "Gerente"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// K6: ningún `code` de skill tiene mayúsculas, espacios ni tildes.
    #[test]
    fn k6_los_codes_de_skills_son_ascii_en_minusculas() {
        assert!(SKILLS.len() >= 80, "hacen falta al menos 80 skills");
        for (code, name, aliases) in SKILLS {
            assert!(
                !code.is_empty()
                    && code.split('-').all(|p| !p.is_empty()
                        && p.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())),
                "code inválido: {code:?}"
            );
            assert!(!name.trim().is_empty());
            for a in *aliases {
                assert!(
                    a.bytes().all(|b| b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b == b'-'
                        || b == b'.'
                        || b == b'+'
                        || b == b'#'),
                    "alias inválido: {a:?}"
                );
            }
        }
        let mut codes: Vec<_> = SKILLS.iter().map(|s| s.0).collect();
        codes.sort_unstable();
        let n = codes.len();
        codes.dedup();
        assert_eq!(n, codes.len(), "hay codes repetidos");
    }
}
