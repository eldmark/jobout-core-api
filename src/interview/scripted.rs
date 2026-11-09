//! A scripted engine for tests of the layers above (sessions, turns, scoring).

use super::*;

/// Returns exactly what it was told: the n-th scripted turn after n candidate answers, and the
/// configured evaluation result.
#[derive(Debug, Clone)]
pub struct ScriptedEngine {
    turns: Vec<NextTurn>,
    evaluation: Result<Evaluation, EngineError>,
}

impl ScriptedEngine {
    pub fn new(turns: Vec<NextTurn>, evaluation: Result<Evaluation, EngineError>) -> Self {
        Self { turns, evaluation }
    }
}

impl InterviewEngine for ScriptedEngine {
    fn next_turn(&self, _: &RecruiterConfig, history: &[Message]) -> Result<NextTurn, EngineError> {
        let answered = history
            .iter()
            .filter(|m| m.speaker == Speaker::Candidate)
            .count();
        self.turns
            .get(answered)
            .cloned()
            .ok_or(EngineError::ScriptExhausted)
    }

    fn evaluate(
        &self,
        _: &RecruiterConfig,
        _: &[Message],
        _: &[RubricCriterion],
    ) -> Result<Evaluation, EngineError> {
        self.evaluation.clone()
    }
}
