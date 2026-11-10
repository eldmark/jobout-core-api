//! Question banks, follow-ups, closings and thresholds, per recruiter code and level.
//!
//! Spanish for ana, martin, laura and carlos; English for sofia. Everything here is static data.

use super::{Level, RecruiterCode};

pub struct Question {
    pub topic: &'static str,
    pub text: &'static str,
}

const fn q(topic: &'static str, text: &'static str) -> Question {
    Question { topic, text }
}

/// Minimum words for an answer to be accepted without a follow-up. Martin is the strictest.
pub fn min_words(code: RecruiterCode) -> usize {
    match code {
        RecruiterCode::Martin => 30,
        RecruiterCode::Carlos => 20,
        _ => 15,
    }
}

/// At most this many follow-ups per main question.
pub const MAX_FOLLOW_UPS: usize = 2;

pub fn questions(code: RecruiterCode, level: Level) -> &'static [Question] {
    use Level::*;
    use RecruiterCode::*;
    match (code, level) {
        (Ana, Junior) => &ANA_JUNIOR,
        (Ana, Startup) => &ANA_STARTUP,
        (Martin, Junior) => &MARTIN_JUNIOR,
        (Martin, Startup) => &MARTIN_STARTUP,
        (Laura, Junior) => &LAURA_JUNIOR,
        (Laura, Startup) => &LAURA_STARTUP,
        (Carlos, Junior) => &CARLOS_JUNIOR,
        (Carlos, Startup) => &CARLOS_STARTUP,
        (Sofia, Junior) => &SOFIA_JUNIOR,
        (Sofia, Startup) => &SOFIA_STARTUP,
    }
}

/// `n` is 0 for the first follow-up of a question and 1 for the second.
pub fn follow_up(code: RecruiterCode, n: usize) -> &'static str {
    let pair: [&str; 2] = match code {
        RecruiterCode::Ana => [
            "Gracias. ¿Puedes darme un ejemplo concreto de una situación real en la que te pasó eso?",
            "Entiendo. ¿Qué hiciste tú en concreto y cómo terminó la historia?",
        ],
        RecruiterCode::Martin => [
            "Eso es demasiado general. Dame un caso concreto: qué herramienta usaste, qué decidiste y por qué.",
            "Todavía falta precisión. ¿Qué números o resultados medibles tuvo esa decisión?",
        ],
        RecruiterCode::Laura => [
            "¿Podrías ponerme un ejemplo real de un proyecto o de un día de trabajo?",
            "¿Y qué resultado tuvo para el equipo o para el cliente?",
        ],
        RecruiterCode::Carlos => [
            "Vale, pero baja a tierra: ¿qué pasó exactamente en un caso real?",
            "¿Qué harías distinto hoy y qué aprendiste de eso?",
        ],
        RecruiterCode::Sofia => [
            "Thanks. Could you give me a concrete example from your own experience?",
            "What exactly did you do, and what was the result?",
        ],
    };
    pair[n.min(1)]
}

pub fn reprompt(code: RecruiterCode) -> &'static str {
    match code {
        RecruiterCode::Sofia => {
            "I did not receive an answer. Please write your answer so we can go on."
        }
        _ => "No recibí tu respuesta. Escríbela con tus palabras para que podamos seguir.",
    }
}

pub fn closing(code: RecruiterCode) -> &'static str {
    match code {
        RecruiterCode::Ana => "Eso es todo por mi parte. Gracias por tu tiempo y por la honestidad de tus respuestas. ¡Mucho ánimo!",
        RecruiterCode::Martin => "Con esto termino la parte técnica. Revisa el desglose: te dirá dónde falta precisión.",
        RecruiterCode::Laura => "Hemos llegado al final de la entrevista. Gracias, ha sido una conversación útil para el equipo.",
        RecruiterCode::Carlos => "Eso es todo, buen trabajo. Mira el resultado con calma y pule lo que quedó flojo.",
        RecruiterCode::Sofia => "That is the end of our interview. Thank you for your time, and good luck with the next step.",
    }
}

static ANA_JUNIOR: [Question; 7] = [
    q("opening", "Hola, soy Ana, de Recursos Humanos. Para empezar, cuéntame quién eres y qué estás buscando en tu primer trabajo."),
    q("motivation", "¿Qué te motiva de este puesto y de la empresa a la que aplicas?"),
    q("teamwork", "Cuéntame de una vez en la que trabajaste en equipo. ¿Cuál fue tu papel?"),
    q("conflict", "¿Recuerdas un desacuerdo con alguien de tu grupo? ¿Cómo lo resolviste?"),
    q("learning", "¿Cuál es lo último que aprendiste por tu cuenta y cómo lo hiciste?"),
    q("strengths", "¿Cuál dirías que es tu mayor fortaleza y en qué quieres mejorar?"),
    q("goals", "¿Dónde te gustaría estar dentro de dos años y qué harás para llegar allí?"),
];

static ANA_STARTUP: [Question; 7] = [
    q(
        "opening",
        "Hola, soy Ana, de Recursos Humanos. Cuéntame quién eres y por qué una startup.",
    ),
    q(
        "motivation",
        "¿Qué te atrae de trabajar en una empresa pequeña, con poca estructura?",
    ),
    q(
        "teamwork",
        "Háblame de un equipo pequeño en el que trabajaste: ¿qué hacías tú y qué hacían los demás?",
    ),
    q(
        "ambiguity",
        "¿Cómo reaccionas cuando no te dicen exactamente qué hay que hacer?",
    ),
    q(
        "ownership",
        "Cuéntame de algo que sacaste adelante sin que nadie te lo pidiera.",
    ),
    q(
        "pressure",
        "¿Cómo manejas las semanas con mucha carga y un plazo de entrega apretado?",
    ),
    q(
        "goals",
        "¿Qué esperas aprender en tu primer año en una startup?",
    ),
];

static MARTIN_JUNIOR: [Question; 6] = [
    q("opening", "Soy Martín, el CTO. Describe el proyecto técnico del que estás más orgulloso y qué parte construiste tú."),
    q("technical_decisions", "¿Qué decisión técnica tomaste en ese proyecto y qué alternativas descartaste?"),
    q("debugging", "Cuéntame el bug más difícil que has resuelto: cómo lo encontraste y cómo lo arreglaste."),
    q("code_quality", "¿Cómo compruebas que tu código funciona antes de entregarlo?"),
    q("data", "Explica cómo diseñarías las tablas de una aplicación sencilla de pedidos y por qué."),
    q("learning_tech", "¿Qué tecnología aprendiste recientemente y qué te dejó claro que no sabías?"),
];

static MARTIN_STARTUP: [Question; 7] = [
    q("opening", "Soy Martín, el CTO. Cuéntame el sistema más grande que has construido y el papel que tuviste."),
    q("technical_decisions", "¿Qué decisión técnica de ese sistema repetirías y cuál cambiarías?"),
    q("tradeoffs", "Dame un caso en el que sacrificaste calidad por velocidad. ¿Qué costo tuvo?"),
    q("scaling", "¿Cómo detectarías que una consulta lenta está frenando a todos los usuarios?"),
    q("incidents", "Cuéntame una caída o un error en producción: qué pasó y cómo se resolvió."),
    q("ownership", "¿Qué parte del código dejarías mejor de lo que la encontraste y por qué?"),
    q("tooling", "¿Qué herramientas de despliegue y monitoreo has usado, y qué harías con ellas en un equipo de tres?"),
];

static LAURA_JUNIOR: [Question; 6] = [
    q("opening", "Hola, soy Laura, gerente de ingeniería. Cuéntame cómo llegaste a esta carrera y qué sabes hacer hoy."),
    q("prioritization", "Si tienes tres tareas para el mismo día, ¿cómo decides cuál va primero?"),
    q("feedback", "Cuéntame de un comentario difícil que recibiste y qué hiciste con él."),
    q("deadlines", "¿Qué haces cuando ves que no llegarás a una fecha de entrega?"),
    q("communication", "Explícame un tema técnico como si yo no fuera del área."),
    q("ownership", "Háblame de una tarea que terminaste de principio a fin y cómo supiste que estaba lista."),
];

static LAURA_STARTUP: [Question; 6] = [
    q("opening", "Hola, soy Laura, gerente de ingeniería. Cuéntame qué has construido y qué quieres construir aquí."),
    q("prioritization", "En una startup todo es urgente. ¿Cómo decides qué hacer primero y qué dejar para después?"),
    q("stakeholders", "Cuéntame de una vez en la que alguien pidió algo que no tenía sentido. ¿Qué hiciste?"),
    q("feedback", "¿Cómo das y cómo recibes críticas sobre el trabajo de un compañero?"),
    q("deadlines", "Describe un plazo que estuvo en riesgo y cómo lo manejaste."),
    q("ownership", "¿Qué significa para ti hacerte responsable de un producto que usa gente real?"),
];

static CARLOS_JUNIOR: [Question; 6] = [
    q("opening", "Qué tal, soy Carlos, tech lead. Cuéntame qué stack manejas y en qué te sientes más cómodo."),
    q("code_review", "¿Qué buscas cuando revisas el código de otra persona, y qué esperas que miren en el tuyo?"),
    q("problem_solving", "Un test falla solo a veces. ¿Cómo investigas qué pasa?"),
    q("mistakes", "Cuéntame un error tuyo que llegó lejos y qué cambió después en tu forma de trabajar."),
    q("initiative", "¿Has propuesto alguna mejora en un proyecto que no era tuyo? Cuéntame cómo fue."),
    q("learning", "¿Cómo te mantienes al día y qué has puesto en práctica de lo que aprendiste?"),
];

static CARLOS_STARTUP: [Question; 7] = [
    q("opening", "Qué tal, soy Carlos, tech lead. Cuéntame qué sueles hacer de principio a fin en un proyecto."),
    q("code_review", "¿Cómo equilibras revisar código con avanzar en lo tuyo cuando el equipo es pequeño?"),
    q("problem_solving", "Producción falla un viernes por la tarde. ¿Qué haces en los primeros veinte minutos?"),
    q("mistakes", "Cuéntame un error que costó tiempo o dinero y qué quedó como lección."),
    q("initiative", "¿Qué proceso del equipo mejorarías si fuera tu decisión y cómo lo harías?"),
    q("pairing", "¿Cómo ayudas a una persona nueva a ponerse al día con el código?"),
    q("learning", "¿Qué te gustaría que el equipo te enseñara en los primeros seis meses?"),
];

static SOFIA_JUNIOR: [Question; 6] = [
    q("opening", "Hi, I am Sofia. To begin, tell me about yourself and what kind of role you are looking for."),
    q("motivation", "What draws you to this role, and what do you want to learn in it?"),
    q("remote_work", "How do you stay organized when you work with people in other time zones?"),
    q("communication", "Tell me about a time you explained something complicated to someone. How did it go?"),
    q("teamwork", "Describe a project you did with other people. What was your part?"),
    q("goals", "Where do you see yourself in two years, and what are you doing to get there?"),
];

static SOFIA_STARTUP: [Question; 6] = [
    q(
        "opening",
        "Hi, I am Sofia. Tell me about yourself and why you want to join a startup.",
    ),
    q(
        "motivation",
        "What makes you want to work in a small company where the rules are not written yet?",
    ),
    q(
        "remote_work",
        "How do you keep your team up to date when everyone works from a different place?",
    ),
    q(
        "problem_solving",
        "Tell me about a problem you solved with very little help. What did you try first?",
    ),
    q(
        "ownership",
        "Describe something you took charge of without being asked. What was the outcome?",
    ),
    q(
        "goals",
        "What do you hope to have achieved after your first year here?",
    ),
];
