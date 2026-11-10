//! Deterministic rule-based engine: the recruiter of the text-only phase.

use super::bank::{self, MAX_FOLLOW_UPS};
use super::text;
use super::*;

/// Rule-based engine. Stateless: everything is recomputed from the history, so the same history
/// always yields exactly the same turn.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeterministicEngine;

/// Where the conversation stands after replaying the candidate's answers.
struct Progress {
    /// Index of the main question currently open.
    question: usize,
    /// Follow-ups already asked on it.
    follow_ups: usize,
    /// What the last candidate message was, if any.
    last: Option<Last>,
}

#[derive(PartialEq, Eq)]
enum Last {
    Empty,
    Insufficient,
    Accepted,
}

fn replay(config: &RecruiterConfig, history: &[Message]) -> Progress {
    let min_words = bank::min_words(config.recruiter);
    let total = bank::questions(config.recruiter, config.level).len();
    let mut p = Progress {
        question: 0,
        follow_ups: 0,
        last: None,
    };
    for m in history.iter().filter(|m| m.speaker == Speaker::Candidate) {
        if p.question >= total {
            break;
        }
        let answer = text::sanitize(&m.text);
        if answer.trim().is_empty() {
            p.last = Some(Last::Empty);
            continue;
        }
        let enough = text::word_count(&answer) >= min_words && text::has_concrete_example(&answer);
        if enough || p.follow_ups >= MAX_FOLLOW_UPS {
            p.question += 1;
            p.follow_ups = 0;
            p.last = Some(Last::Accepted);
        } else {
            p.follow_ups += 1;
            p.last = Some(Last::Insufficient);
        }
    }
    p
}

impl InterviewEngine for DeterministicEngine {
    fn next_turn(
        &self,
        config: &RecruiterConfig,
        history: &[Message],
    ) -> Result<NextTurn, EngineError> {
        let bank = bank::questions(config.recruiter, config.level);
        let p = replay(config, history);
        let Some(current) = bank.get(p.question) else {
            return Ok(NextTurn {
                message: bank::closing(config.recruiter).to_string(),
                kind: TurnKind::Closing,
                topic: None,
                done: true,
            });
        };
        let (message, kind) = match p.last {
            None | Some(Last::Accepted) => (current.text, TurnKind::Question),
            Some(Last::Empty) => (bank::reprompt(config.recruiter), TurnKind::Reprompt),
            Some(Last::Insufficient) => (
                bank::follow_up(config.recruiter, p.follow_ups - 1),
                TurnKind::FollowUp,
            ),
        };
        Ok(NextTurn {
            message: message.to_string(),
            kind,
            topic: Some(current.topic.to_string()),
            done: false,
        })
    }

    fn evaluate(
        &self,
        config: &RecruiterConfig,
        transcript: &[Message],
        criteria: &[RubricCriterion],
    ) -> Result<Evaluation, EngineError> {
        evaluation::evaluate(config, transcript, criteria)
    }
}
