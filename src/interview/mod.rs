//! Interview engine (JB-38): a pure library, with no database, HTTP, network or clock.
//!
//! The engine decides what the recruiter says next and scores a finished transcript against a
//! rubric. It only ever sees the history, the recruiter configuration and plain rubric
//! criteria: never a name, an email or an id of a person (case X7). The input types below are
//! the whole surface, and `tests/interview_engine.rs` pins it by destructuring every one of them.

mod bank;
mod deterministic;
mod evaluation;
mod scripted;
mod text;

pub use deterministic::DeterministicEngine;
pub use scripted::ScriptedEngine;

/// Who wrote a message of the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Recruiter,
    Candidate,
}

/// One message of the conversation. Text only: no identity of any kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub speaker: Speaker,
    pub text: String,
}

impl Message {
    pub fn recruiter(text: impl Into<String>) -> Self {
        Self {
            speaker: Speaker::Recruiter,
            text: text.into(),
        }
    }

    pub fn candidate(text: impl Into<String>) -> Self {
        Self {
            speaker: Speaker::Candidate,
            text: text.into(),
        }
    }
}

/// The recruiters, referred to by their stable `code` (never by a database id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecruiterCode {
    Ana,
    Martin,
    Laura,
    Carlos,
    Sofia,
}

impl RecruiterCode {
    pub const ALL: [RecruiterCode; 5] = [
        RecruiterCode::Ana,
        RecruiterCode::Martin,
        RecruiterCode::Laura,
        RecruiterCode::Carlos,
        RecruiterCode::Sofia,
    ];

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "ana" => Some(Self::Ana),
            "martin" => Some(Self::Martin),
            "laura" => Some(Self::Laura),
            "carlos" => Some(Self::Carlos),
            "sofia" => Some(Self::Sofia),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Ana => "ana",
            Self::Martin => "martin",
            Self::Laura => "laura",
            Self::Carlos => "carlos",
            Self::Sofia => "sofia",
        }
    }

    pub fn language(self) -> Language {
        match self {
            Self::Sofia => Language::En,
            _ => Language::Es,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Es,
    En,
}

/// Levels that have a question bank (the other three exist in the catalog but are not available).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Junior,
    Startup,
}

impl Level {
    pub const AVAILABLE: [Level; 2] = [Level::Junior, Level::Startup];

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "junior" => Some(Self::Junior),
            "startup" => Some(Self::Startup),
            _ => None,
        }
    }
}

/// Recruiter configuration: which recruiter runs the interview and at which level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecruiterConfig {
    pub recruiter: RecruiterCode,
    pub level: Level,
}

/// One criterion of a rubric version, as plain data (the engine does not touch the database).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubricCriterion {
    pub code: String,
    pub name: String,
    pub max_points: u32,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    Question,
    FollowUp,
    /// The last answer was empty: the recruiter asks the candidate to answer.
    Reprompt,
    Closing,
}

/// What the recruiter says next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextTurn {
    pub message: String,
    pub kind: TurnKind,
    /// Topic of the main question this turn belongs to (`None` for the closing message).
    pub topic: Option<String>,
    /// `true` when the interview is over (the message is the closing).
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionResult {
    pub code: String,
    pub name: String,
    pub points: u32,
    pub max_points: u32,
    /// Never empty. May quote the candidate, at most 80 characters.
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Improvement {
    pub criterion_code: String,
    pub text: String,
    /// How many points this criterion could still gain; improvements are sorted by it, descending.
    pub potential_points: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// Always the sum of `breakdown[..].points`.
    pub score: u32,
    pub clarity: u32,
    pub breakdown: Vec<CriterionResult>,
    /// Two or three, most valuable first.
    pub improvements: Vec<Improvement>,
    /// The text engine does not invent a confidence.
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// The transcript has no candidate answer: it is not evaluated (not a 0).
    NoAnswers,
    /// No criteria, or their `max_points` do not sum to 100.
    InvalidRubric,
    /// A `ScriptedEngine` ran out of scripted turns.
    ScriptExhausted,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAnswers => write!(f, "the transcript has no candidate answers"),
            Self::InvalidRubric => {
                write!(f, "the rubric criteria must be non-empty and sum to 100")
            }
            Self::ScriptExhausted => write!(f, "the scripted engine has no more turns"),
        }
    }
}

impl std::error::Error for EngineError {}

pub trait InterviewEngine {
    /// The next thing the recruiter says, given the conversation so far (empty history gives the
    /// opening question). Pure: the same history always gives the same turn.
    fn next_turn(
        &self,
        config: &RecruiterConfig,
        history: &[Message],
    ) -> Result<NextTurn, EngineError>;

    /// Score a transcript against rubric criteria.
    fn evaluate(
        &self,
        config: &RecruiterConfig,
        transcript: &[Message],
        criteria: &[RubricCriterion],
    ) -> Result<Evaluation, EngineError>;
}
