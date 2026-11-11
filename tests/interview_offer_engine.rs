//! Interview engine tailored to a published offer (Phase 5b, step 9, HU-31; cases PR1/PR2/PR6 at
//! the engine level). Pure library tests: no database, no HTTP. Written before the code.
//!
//! Contract (decision 12, minimal option): the engine receives only the offer title and the names
//! of the required skills and languages (an [`OfferFocus`]); the recruiter personas, the bank and
//! the follow-ups stay as they are, and the same input always gives the same turn.

use jobout_core_api::interview::*;

const GOOD_ES: &str = "En mi proyecto final de la universidad usamos Rust con PostgreSQL y yo me encargué de las consultas lentas, porque la página tardaba 800 ms en responder. Primero medí el problema, después añadí un índice y reescribí dos consultas, y como resultado el tiempo bajó a 200 ms y atendimos a 300 usuarios a la vez.";
const GOOD_EN: &str = "In my final project at university we used Rust with PostgreSQL and I owned the slow queries, because the page took 800 ms to answer. First I measured the problem, then I added an index and rewrote two queries, and as a result the time dropped to 200 ms and we served 300 users at once.";
const VAGUE_ES: &str =
    "Pues sí, me gusta mucho trabajar en equipo y aprender cosas nuevas siempre.";

fn cfg(r: RecruiterCode, l: Level) -> RecruiterConfig {
    RecruiterConfig {
        recruiter: r,
        level: l,
    }
}

fn focus(title: &str, skills: &[&str], languages: &[&str]) -> OfferFocus {
    OfferFocus {
        title: title.to_string(),
        skills: skills.iter().map(|s| s.to_string()).collect(),
        languages: languages.iter().map(|s| s.to_string()).collect(),
    }
}

/// Plays a whole interview with good answers; returns the recruiter's turns.
fn play(
    e: &dyn InterviewEngine,
    c: &RecruiterConfig,
    f: Option<&OfferFocus>,
    answer: &str,
) -> Vec<NextTurn> {
    let mut history = Vec::new();
    let mut turns = Vec::new();
    for _ in 0..200 {
        let t = match f {
            Some(f) => e.next_turn_for_offer(c, f, &history),
            None => e.next_turn(c, &history),
        }
        .expect("next turn");
        history.push(Message::recruiter(t.message.clone()));
        let done = t.done;
        turns.push(t);
        if done {
            return turns;
        }
        history.push(Message::candidate(answer));
    }
    panic!("the interview never ended");
}

fn questions(turns: &[NextTurn]) -> Vec<String> {
    turns
        .iter()
        .filter(|t| t.kind == TurnKind::Question)
        .map(|t| t.message.clone())
        .collect()
}

// ---- PR1: the questions depend on the offer ----

#[test]
fn pr1_the_opening_names_the_offer_title_for_every_recruiter_and_level() {
    for r in RecruiterCode::ALL {
        for l in Level::AVAILABLE {
            let c = cfg(r, l);
            let f = focus("Analista de datos", &["Python"], &[]);
            let plain = DeterministicEngine.next_turn(&c, &[]).unwrap();
            let tailored = DeterministicEngine
                .next_turn_for_offer(&c, &f, &[])
                .unwrap();
            assert!(
                tailored.message.contains("Analista de datos"),
                "{r:?} {l:?}"
            );
            assert_ne!(tailored.message, plain.message);
            // The persona greeting is intact: the tailored opening starts with the plain one.
            assert!(tailored.message.starts_with(&plain.message), "{r:?} {l:?}");
            assert_eq!(tailored.kind, TurnKind::Question);
            assert!(!tailored.done);
        }
    }
}

#[test]
fn pr1_a_required_skill_and_a_language_become_a_question_each() {
    let f = focus("Analista de datos", &["Python", "SQL"], &["Inglés"]);
    let turns = play(
        &DeterministicEngine,
        &cfg(RecruiterCode::Ana, Level::Junior),
        Some(&f),
        GOOD_ES,
    );
    let qs = questions(&turns);
    for hint in ["Python", "SQL", "Inglés"] {
        assert_eq!(
            qs.iter().filter(|q| q.contains(hint)).count(),
            1,
            "{hint} must be asked exactly once: {qs:?}"
        );
    }
    assert!(turns.last().unwrap().done);
    assert_eq!(turns.last().unwrap().kind, TurnKind::Closing);
}

#[test]
fn pr1_the_whole_base_bank_is_still_asked_in_order() {
    let c = cfg(RecruiterCode::Laura, Level::Startup);
    let base = questions(&play(&DeterministicEngine, &c, None, GOOD_ES));
    let f = focus(
        "Diseñadora",
        &["Figma", "Photoshop"],
        &["Inglés", "Francés"],
    );
    let tailored = questions(&play(&DeterministicEngine, &c, Some(&f), GOOD_ES));
    assert_eq!(tailored.len(), base.len() + 4);
    // Removing the offer-specific questions leaves the base bank, in order; the first one is the
    // base opening plus the offer sentence.
    let rest: Vec<&String> = tailored
        .iter()
        .filter(|q| {
            !["Figma", "Photoshop", "Inglés", "Francés"]
                .iter()
                .any(|h| q.contains(h))
        })
        .collect();
    assert_eq!(rest.len(), base.len());
    assert!(rest[0].starts_with(&base[0]));
    for (a, b) in rest.iter().zip(&base).skip(1) {
        assert_eq!(*a, b);
    }
}

#[test]
fn pr1_without_requirements_only_the_opening_changes() {
    let c = cfg(RecruiterCode::Carlos, Level::Junior);
    let base = questions(&play(&DeterministicEngine, &c, None, GOOD_ES));
    let f = focus("Cajero", &[], &[]);
    let tailored = questions(&play(&DeterministicEngine, &c, Some(&f), GOOD_ES));
    assert_eq!(tailored.len(), base.len());
    assert!(tailored[0].contains("Cajero"));
    assert_eq!(tailored[1..], base[1..]);
}

#[test]
fn pr1_a_blank_title_and_no_requirements_is_the_plain_interview() {
    let c = cfg(RecruiterCode::Ana, Level::Junior);
    let f = focus("   ", &[], &[]);
    assert_eq!(
        play(&DeterministicEngine, &c, Some(&f), GOOD_ES),
        play(&DeterministicEngine, &c, None, GOOD_ES)
    );
}

#[test]
fn pr1_it_is_deterministic() {
    let c = cfg(RecruiterCode::Martin, Level::Startup);
    let f = focus("Backend", &["Rust", "PostgreSQL"], &["Inglés"]);
    let a = play(&DeterministicEngine, &c, Some(&f), GOOD_ES);
    let b = play(&DeterministicEngine, &c, Some(&f), GOOD_ES);
    assert_eq!(a, b);
}

#[test]
fn pr1_follow_ups_still_work_on_an_offer_question() {
    let c = cfg(RecruiterCode::Ana, Level::Junior);
    let f = focus("Analista", &["Python"], &[]);
    let e = DeterministicEngine;
    let mut h = vec![Message::recruiter(
        e.next_turn_for_offer(&c, &f, &[]).unwrap().message,
    )];
    h.push(Message::candidate(GOOD_ES));
    let skill_q = e.next_turn_for_offer(&c, &f, &h).unwrap();
    assert!(skill_q.message.contains("Python"));
    assert_eq!(skill_q.kind, TurnKind::Question);
    h.push(Message::recruiter(skill_q.message.clone()));
    h.push(Message::candidate(VAGUE_ES));
    let again = e.next_turn_for_offer(&c, &f, &h).unwrap();
    assert_eq!(again.kind, TurnKind::FollowUp);
    assert_eq!(again.topic, skill_q.topic);
}

#[test]
fn pr1_there_are_at_most_three_skill_and_two_language_questions() {
    let c = cfg(RecruiterCode::Ana, Level::Junior);
    let skills = ["S1x", "S2x", "S3x", "S4x", "S5x"];
    let langs = ["L1x", "L2x", "L3x"];
    let f = focus("Puesto", &skills, &langs);
    let qs = questions(&play(&DeterministicEngine, &c, Some(&f), GOOD_ES));
    let n = |p: &str| qs.iter().filter(|q| q.contains(p)).count();
    assert_eq!(n("S1x") + n("S2x") + n("S3x"), 3);
    assert_eq!(n("S4x") + n("S5x"), 0);
    assert_eq!(n("L1x") + n("L2x"), 2);
    assert_eq!(n("L3x"), 0);
}

#[test]
fn pr1_the_english_recruiter_asks_in_english() {
    let c = cfg(RecruiterCode::Sofia, Level::Junior);
    let f = focus("Data Analyst", &["Python"], &["Español"]);
    let qs = questions(&play(&DeterministicEngine, &c, Some(&f), GOOD_EN));
    let extra: Vec<&String> = qs
        .iter()
        .filter(|q| q.contains("Python") || q.contains("Español"))
        .collect();
    assert_eq!(extra.len(), 2);
    for q in extra {
        assert!(!q.contains("Cuéntame") && !q.contains("¿"), "{q}");
    }
    assert!(qs[0].contains("Data Analyst"));
}

// ---- PR6: only the title and the requirements reach the engine ----

#[test]
fn pr6_the_title_is_cleaned_and_capped() {
    let c = cfg(RecruiterCode::Ana, Level::Junior);
    let largo = "X".repeat(300);
    let f = focus(&format!("  Analista \n\n  senior {largo}"), &[], &[]);
    let t = DeterministicEngine
        .next_turn_for_offer(&c, &f, &[])
        .unwrap();
    assert!(t.message.contains("Analista senior X"));
    assert!(!t.message.contains('\n'));
    assert!(!t.message.contains(&"X".repeat(120)));
}

#[test]
fn pr6_offer_focus_carries_no_company_or_person_fields() {
    // Exhaustive destructuring: a new field (a company, an id, a name...) stops this compiling,
    // so it has to be reviewed against the privacy rule (X7 of phase 3, PR6).
    let OfferFocus {
        title,
        skills,
        languages,
    } = focus("t", &["s"], &["l"]);
    let _: (String, Vec<String>, Vec<String>) = (title, skills, languages);
}

#[test]
fn the_default_trait_method_ignores_the_offer_for_engines_that_do_not_know_it() {
    let c = cfg(RecruiterCode::Ana, Level::Junior);
    let scripted = ScriptedEngine::new(
        vec![NextTurn {
            message: "hola".into(),
            kind: TurnKind::Question,
            topic: None,
            done: false,
        }],
        Err(EngineError::NoAnswers),
    );
    let t = scripted
        .next_turn_for_offer(&c, &focus("Algo", &["X"], &[]), &[])
        .unwrap();
    assert_eq!(t.message, "hola");
}
