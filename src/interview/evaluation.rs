//! Rubric evaluation: per-criterion breakdown from plain text features.

use super::text::{self, Signals};
use super::*;

/// One candidate answer with the recruiter message that preceded it.
struct Answer {
    question: Option<String>,
    text: String,
}

fn answers(transcript: &[Message]) -> Vec<Answer> {
    let mut out = Vec::new();
    let mut question: Option<String> = None;
    for m in transcript {
        match m.speaker {
            Speaker::Recruiter => question = Some(m.text.clone()),
            Speaker::Candidate => {
                let clean = text::sanitize(&m.text);
                if !clean.is_empty() {
                    out.push(Answer {
                        question: question.clone(),
                        text: clean,
                    });
                }
            }
        }
    }
    out
}

fn mean(xs: impl Iterator<Item = f64>) -> f64 {
    let (n, sum) = xs.fold((0usize, 0.0), |(n, s), x| (n + 1, s + x));
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

fn specificity(a: &Answer) -> f64 {
    clamp01(text::signals(&a.text).total() as f64 / 3.0)
}

fn structure(a: &Answer) -> f64 {
    text::structure_markers(&a.text) as f64 / 3.0
}

fn relevance(a: &Answer) -> f64 {
    let Some(q) = &a.question else { return 0.5 };
    let q = text::stems(q);
    if q.is_empty() {
        return 0.5;
    }
    let overlap = q.intersection(&text::stems(&a.text)).count();
    clamp01(overlap as f64 / q.len().min(2) as f64)
}

/// Clarity of one answer in 0..=1: sentences of a readable length, and not just repetition.
fn clarity_of(a: &Answer) -> f64 {
    let len = text::mean_sentence_words(&a.text);
    let length_factor = if len < 8.0 {
        len / 8.0
    } else if len <= 25.0 {
        1.0
    } else {
        (25.0 / len).max(0.3)
    };
    let words = text::word_count(&a.text).max(1) as f64;
    let variety = clamp01(text::distinct_tokens(&a.text) as f64 / words * 2.0);
    length_factor * variety
}

fn depth(a: &Answer) -> f64 {
    let words = clamp01(text::distinct_tokens(&a.text) as f64 / 40.0);
    let reasoning = clamp01(text::reasoning_markers(&a.text) as f64 / 2.0);
    0.6 * words + 0.4 * reasoning
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Band {
    High,
    Mid,
    Low,
}

fn band(ratio: f64) -> Band {
    if ratio >= 0.7 {
        Band::High
    } else if ratio >= 0.35 {
        Band::Mid
    } else {
        Band::Low
    }
}

fn why_text(lang: Language, code: &str, b: Band) -> &'static str {
    use Band::*;
    use Language::*;
    match (lang, code, b) {
        (Es, "especificidad", High) => "Das cifras, herramientas o resultados concretos.",
        (Es, "especificidad", Mid) => {
            "Hay algún dato concreto, pero faltan cifras o resultados medibles."
        }
        (Es, "especificidad", Low) => "Casi no hay cifras, nombres de herramientas ni resultados.",
        (Es, "estructura", High) => "Tus respuestas siguen situación, acción y resultado.",
        (Es, "estructura", Mid) => {
            "Aparece una parte de situación, acción o resultado, pero no las tres."
        }
        (Es, "estructura", Low) => "No se distingue la situación, lo que hiciste ni el resultado.",
        (Es, "relevancia", High) => "Respondes a lo que se preguntó.",
        (Es, "relevancia", Mid) => "Tocas la pregunta solo en parte.",
        (Es, "relevancia", Low) => "Las respuestas se alejan de las preguntas.",
        (Es, "claridad", High) => "Las frases tienen una longitud cómoda de leer.",
        (Es, "claridad", Mid) => {
            "Se entiende, pero las frases son muy cortas, muy largas o repetitivas."
        }
        (Es, "claridad", Low) => {
            "Las respuestas son demasiado breves o repetitivas para entenderse bien."
        }
        (Es, "profundidad", High) => "Explicas el porqué de tus decisiones.",
        (Es, "profundidad", Mid) => "Explicas un poco, pero sin razonar las decisiones.",
        (Es, "profundidad", Low) => "Las respuestas se quedan en la superficie.",
        (Es, _, High) => "Este criterio se cumple bien en tus respuestas.",
        (Es, _, Mid) => "Este criterio se cumple en parte.",
        (Es, _, Low) => "Este criterio casi no se cumple.",
        (En, "especificidad", High) => "You give concrete figures, tools or results.",
        (En, "especificidad", Mid) => {
            "There is some concrete detail, but figures or measurable results are missing."
        }
        (En, "especificidad", Low) => "There are almost no figures, tool names or results.",
        (En, "estructura", High) => "Your answers follow situation, action and result.",
        (En, "estructura", Mid) => {
            "One part of situation, action or result shows up, but not all three."
        }
        (En, "estructura", Low) => "The situation, your action and the result are not told apart.",
        (En, "relevancia", High) => "You answer what was asked.",
        (En, "relevancia", Mid) => "You touch the question only in part.",
        (En, "relevancia", Low) => "The answers drift away from the questions.",
        (En, "claridad", High) => "Your sentences are a comfortable length to read.",
        (En, "claridad", Mid) => {
            "It is understandable, but sentences are very short, very long or repetitive."
        }
        (En, "claridad", Low) => "Answers are too short or repetitive to follow well.",
        (En, "profundidad", High) => "You explain the reasons behind your choices.",
        (En, "profundidad", Mid) => "You explain a little, but do not reason through the choices.",
        (En, "profundidad", Low) => "The answers stay on the surface.",
        (En, _, High) => "This criterion is well met in your answers.",
        (En, _, Mid) => "This criterion is partly met.",
        (En, _, Low) => "This criterion is barely met.",
    }
}

fn improvement_text(lang: Language, code: &str) -> &'static str {
    match (lang, code) {
        (Language::Es, "especificidad") => {
            "Añade cifras, nombres de herramientas y el resultado medible de lo que hiciste."
        }
        (Language::Es, "estructura") => {
            "Cuenta cada ejemplo en orden: la situación, lo que hiciste tú y el resultado."
        }
        (Language::Es, "relevancia") => {
            "Responde primero justo lo que se pregunta y luego añade el contexto."
        }
        (Language::Es, "claridad") => {
            "Usa frases de unas 10 a 25 palabras y evita repetir las mismas ideas."
        }
        (Language::Es, "profundidad") => {
            "Explica por qué tomaste cada decisión y qué alternativas descartaste."
        }
        (Language::Es, _) => "Desarrolla más este punto con un ejemplo real.",
        (Language::En, "especificidad") => {
            "Add figures, tool names and the measurable result of what you did."
        }
        (Language::En, "estructura") => {
            "Tell each example in order: the situation, what you did, and the result."
        }
        (Language::En, "relevancia") => "Answer exactly what is asked first, then add the context.",
        (Language::En, "claridad") => {
            "Use sentences of about 10 to 25 words and avoid repeating the same ideas."
        }
        (Language::En, "profundidad") => {
            "Explain why you made each decision and which options you dropped."
        }
        (Language::En, _) => "Develop this point further with a real example.",
    }
}

/// A short quote from the answers that shows a concrete detail, or none.
fn concrete_quote(answers: &[Answer]) -> Option<String> {
    answers.iter().find_map(|a| {
        text::sentences(&a.text)
            .into_iter()
            .find(|s| text::signals(s) != Signals::default())
            .map(|s| text::excerpt(&s))
    })
}

pub fn evaluate(
    config: &RecruiterConfig,
    transcript: &[Message],
    criteria: &[RubricCriterion],
) -> Result<Evaluation, EngineError> {
    if criteria.is_empty() || criteria.iter().map(|c| c.max_points).sum::<u32>() != 100 {
        return Err(EngineError::InvalidRubric);
    }
    let answers = answers(transcript);
    if answers.is_empty() {
        return Err(EngineError::NoAnswers);
    }
    let lang = config.recruiter.language();

    let ratios = [
        ("especificidad", mean(answers.iter().map(specificity))),
        ("estructura", mean(answers.iter().map(structure))),
        ("relevancia", mean(answers.iter().map(relevance))),
        ("claridad", mean(answers.iter().map(clarity_of))),
        ("profundidad", mean(answers.iter().map(depth))),
    ];
    let overall = mean(ratios.iter().map(|(_, r)| *r));
    let ratio_of = |code: &str| {
        ratios
            .iter()
            .find(|(c, _)| *c == code)
            .map_or(overall, |(_, r)| *r)
    };
    let clarity = (ratio_of("claridad") * 100.0).round() as u32;
    let quote = concrete_quote(&answers);

    let breakdown: Vec<CriterionResult> = criteria
        .iter()
        .map(|c| {
            let ratio = clamp01(ratio_of(&c.code));
            let points = ((c.max_points as f64 * ratio).round() as u32).min(c.max_points);
            let mut why = format!(
                "{} ({}/{}): {}",
                c.name,
                points,
                c.max_points,
                why_text(lang, &c.code, band(ratio))
            );
            if c.code == "especificidad" {
                if let Some(q) = &quote {
                    let label = if lang == Language::En {
                        "Example"
                    } else {
                        "Ejemplo"
                    };
                    why.push_str(&format!(" {label}: «{q}»"));
                }
            }
            CriterionResult {
                code: c.code.clone(),
                name: c.name.clone(),
                points,
                max_points: c.max_points,
                why,
            }
        })
        .collect();

    let score = breakdown.iter().map(|b| b.points).sum();

    // Most points still available first; ties keep the rubric's order (stable sort).
    let mut order: Vec<&CriterionResult> = breakdown.iter().collect();
    order.sort_by_key(|b| std::cmp::Reverse(b.max_points - b.points));
    let improvements = order
        .into_iter()
        .take(3)
        .map(|b| Improvement {
            criterion_code: b.code.clone(),
            text: improvement_text(lang, &b.code).to_string(),
            potential_points: b.max_points - b.points,
        })
        .collect::<Vec<_>>();

    Ok(Evaluation {
        score,
        clarity,
        breakdown,
        improvements,
        confidence: None,
    })
}
