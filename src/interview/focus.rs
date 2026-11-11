//! What the engine may know about the offer a practice session is tailored to (HU-31, decision 12
//! of phase 5, minimal option): the title and the *names* of the required skills and languages.
//! Never the company, never an id, never anything about the person (X7 of phase 3, case PR6).

use super::bank::{self, Question};
use super::{Language, RecruiterCode};

/// Skills asked about, at most (the first ones, in the order given).
pub const MAX_SKILL_QUESTIONS: usize = 3;
/// Languages asked about, at most.
pub const MAX_LANGUAGE_QUESTIONS: usize = 2;
/// Longest title or requirement name the recruiter will repeat, in characters.
const MAX_TEXT_CHARS: usize = 80;

/// Plain data, owned: the title of the offer and the names of its required skills and languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferFocus {
    pub title: String,
    pub skills: Vec<String>,
    pub languages: Vec<String>,
}

/// Collapses whitespace and caps the length, so a long or multi-line title cannot take over a turn.
fn clean(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn cleaned(items: &[String], max: usize) -> Vec<String> {
    items
        .iter()
        .map(|s| clean(s))
        .filter(|s| !s.is_empty())
        .take(max)
        .collect()
}

/// An owned question: the base bank is `&'static`, the offer ones are built per session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub topic: String,
    pub text: String,
}

impl From<&Question> for Planned {
    fn from(q: &Question) -> Self {
        Self {
            topic: q.topic.to_string(),
            text: q.text.to_string(),
        }
    }
}

/// The questions of a session: the base bank of the recruiter and level, and, when there is an
/// offer, the base opening with the offer sentence, one question per required skill and per
/// language right after it, and then the rest of the base bank, untouched and in order.
pub fn plan(base: &[Question], code: RecruiterCode, focus: Option<&OfferFocus>) -> Vec<Planned> {
    let mut out: Vec<Planned> = base.iter().map(Planned::from).collect();
    let Some(focus) = focus else { return out };
    let en = code.language() == Language::En;
    let title = clean(&focus.title);
    if !title.is_empty() {
        if let Some(opening) = out.first_mut() {
            opening.text = format!("{} {}", opening.text, bank::offer_intro(code, &title));
        }
    }
    let extra = cleaned(&focus.skills, MAX_SKILL_QUESTIONS)
        .into_iter()
        .map(|s| Planned {
            topic: "offer_skill".into(),
            text: bank::skill_question(en, &s),
        })
        .chain(
            cleaned(&focus.languages, MAX_LANGUAGE_QUESTIONS)
                .into_iter()
                .map(|l| Planned {
                    topic: "offer_language".into(),
                    text: bank::language_question(en, &l),
                }),
        );
    out.splice(1..1, extra);
    out
}
