//! Data layer of the interview sessions (JB-45): every SQL statement that touches
//! `interview_sessions` and `interview_turns` lives here, so the routes (and the next steps:
//! turns, finish) share one implementation.
//!
//! Rules this module keeps:
//!
//! - Every read or write of a session is scoped to its owner (`applicant_id`): a session of
//!   somebody else is indistinguishable from one that does not exist.
//! - Turn text is **always** encrypted before it reaches the database ([`insert_turn`]) and
//!   decrypted only when it is read back ([`turns_of`], [`last_recruiter_turn`]).
//! - No SQL is built by interpolating a value: everything goes through bind parameters. The only
//!   text spliced into a statement are the constants of this file.
//! - Errors carry no data: [`StoreError`] says which kind of failure it was, never the text of the
//!   database error or any turn.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, RuntimeErr, SqlErr, Statement,
    TransactionTrait, Value,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    crypto::PiiKeys,
    interview::{
        closing_message,
        store_crypto::{decrypt_turn_text, encrypt_turn_text},
        InterviewEngine, Level, Message, RecruiterCode, RecruiterConfig, RubricCriterion, Speaker,
        TurnKind,
    },
    quota::{self, QuotaPolicy},
};

/// A session paused for longer than this is abandoned by the cleanup task.
pub const PAUSED_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// ISO 8601 in UTC with microseconds, e.g. `2026-11-09T12:00:00.123456Z`.
fn iso_sql(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')")
}

/// What can go wrong. Never carries the text of the database error or of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Db,
    Crypto,
    /// The recruiter has no current rubric version (a broken catalog).
    NoRubric,
    /// A row did not have the shape the code expects.
    Corrupt,
    /// The engine could not evaluate the session (or returned something out of contract).
    Engine,
}

impl From<DbErr> for StoreError {
    fn from(_: DbErr) -> Self {
        StoreError::Db
    }
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// The kind that is persisted for what the engine said. The engine's `Reprompt` (the answer was
/// empty, please answer) is stored as a plain `question`: the schema has no `reprompt` kind.
pub fn persisted_kind(kind: TurnKind) -> &'static str {
    match kind {
        TurnKind::Question | TurnKind::Reprompt => "question",
        TurnKind::FollowUp => "follow_up",
        TurnKind::Closing => "closing",
    }
}

fn speaker_sql(speaker: Speaker) -> &'static str {
    match speaker {
        Speaker::Recruiter => "recruiter",
        Speaker::Candidate => "applicant",
    }
}

// ------------------------------------------------------------------ views

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Turn {
    pub seq: i32,
    pub speaker: String,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecruiterRef {
    pub id: String,
    pub display_name: String,
    pub role_title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LevelRef {
    pub id: String,
    pub code: String,
    pub name: String,
}

/// `Interview` of the contract.
#[derive(Debug, Clone, Serialize)]
pub struct Interview {
    pub id: String,
    pub status: String,
    pub recruiter: RecruiterRef,
    pub level: LevelRef,
    pub created_at: String,
    pub turns: Vec<Turn>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SummaryRecruiter {
    pub id: String,
    pub display_name: String,
}

/// `InterviewSummary` of the contract: no transcript.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: String,
    pub status: String,
    pub level: LevelRef,
    pub recruiter: SummaryRecruiter,
    pub score: Option<i32>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecruiterInfo {
    pub id: Uuid,
    pub code: String,
}

/// A level as seen by one person.
#[derive(Debug, Clone)]
pub struct LevelGate {
    pub id: Uuid,
    pub code: String,
    pub available: bool,
    /// First level, or a progress row with `unlocked_at` for this person.
    pub unlocked: bool,
}

// ------------------------------------------------------------------ people, catalog

/// The `applicants.id` of a user, if the account has one.
pub async fn applicant_id<C: ConnectionTrait>(
    db: &C,
    user_id: &str,
) -> Result<Option<Uuid>, StoreError> {
    let row = db
        .query_one(stmt(
            "SELECT id FROM applicants WHERE user_id = $1::uuid",
            vec![user_id.to_string().into()],
        ))
        .await?;
    Ok(row.map(|r| r.try_get_by_index(0)).transpose()?)
}

/// An active recruiter by id.
pub async fn find_recruiter<C: ConnectionTrait>(
    db: &C,
    id: Uuid,
) -> Result<Option<RecruiterInfo>, StoreError> {
    let row = db
        .query_one(stmt(
            "SELECT id, code FROM ai_recruiters WHERE id = $1 AND active",
            vec![id.into()],
        ))
        .await?;
    row.map(|r| {
        Ok(RecruiterInfo {
            id: r.try_get_by_index(0)?,
            code: r.try_get_by_index(1)?,
        })
    })
    .transpose()
    .map_err(|_: DbErr| StoreError::Corrupt)
}

/// An active level by id, with whether `applicant` has it unlocked: the first level always is;
/// any other needs a progress row with `unlocked_at`.
pub async fn find_level<C: ConnectionTrait>(
    db: &C,
    applicant: Uuid,
    id: Uuid,
) -> Result<Option<LevelGate>, StoreError> {
    let row = db
        .query_one(stmt(
            "SELECT l.id, l.code, l.available, \
                    (l.previous_level_id IS NULL OR EXISTS ( \
                        SELECT 1 FROM applicant_level_progress p \
                        WHERE p.applicant_id = $2 AND p.interview_level_id = l.id \
                          AND p.unlocked_at IS NOT NULL)) \
             FROM interview_levels l WHERE l.id = $1 AND l.active",
            vec![id.into(), applicant.into()],
        ))
        .await?;
    row.map(|r| {
        Ok(LevelGate {
            id: r.try_get_by_index(0)?,
            code: r.try_get_by_index(1)?,
            available: r.try_get_by_index(2)?,
            unlocked: r.try_get_by_index(3)?,
        })
    })
    .transpose()
    .map_err(|_: DbErr| StoreError::Corrupt)
}

// ------------------------------------------------------------------ turns

/// Persist one turn. The text is encrypted here, always; callers never pass ciphertext.
pub async fn insert_turn<C: ConnectionTrait>(
    db: &C,
    keys: &PiiKeys,
    session_id: Uuid,
    seq: i32,
    speaker: Speaker,
    kind: &str,
    text: &str,
) -> Result<(), StoreError> {
    let cifrado = encrypt_turn_text(keys, text).map_err(|_| StoreError::Crypto)?;
    db.execute(stmt(
        "INSERT INTO interview_turns (id, interview_session_id, seq, speaker, kind, transcript) \
         VALUES ($1, $2, $3, $4::interview_speaker, $5, $6)",
        vec![
            migration::new_id().into(),
            session_id.into(),
            seq.into(),
            speaker_sql(speaker).to_string().into(),
            kind.to_string().into(),
            cifrado.into(),
        ],
    ))
    .await?;
    Ok(())
}

fn turn_from_row(keys: &PiiKeys, r: &sea_orm::QueryResult) -> Result<Turn, StoreError> {
    let cifrado: Vec<u8> = r.try_get_by_index(3).map_err(|_| StoreError::Corrupt)?;
    Ok(Turn {
        seq: r.try_get_by_index(0).map_err(|_| StoreError::Corrupt)?,
        speaker: r.try_get_by_index(1).map_err(|_| StoreError::Corrupt)?,
        kind: r.try_get_by_index(2).map_err(|_| StoreError::Corrupt)?,
        text: decrypt_turn_text(keys, &cifrado).map_err(|_| StoreError::Crypto)?,
    })
}

/// All the turns of a session, decrypted, in `seq` order. The caller must already have checked
/// that the session is the person's.
pub async fn turns_of<C: ConnectionTrait>(
    db: &C,
    keys: &PiiKeys,
    session_id: Uuid,
) -> Result<Vec<Turn>, StoreError> {
    let filas = db
        .query_all(stmt(
            "SELECT seq, speaker::text, kind, transcript FROM interview_turns \
             WHERE interview_session_id = $1 ORDER BY seq",
            vec![session_id.into()],
        ))
        .await?;
    filas.iter().map(|r| turn_from_row(keys, r)).collect()
}

/// The last turn the recruiter said (highest `seq` with `speaker = recruiter`), decrypted.
pub async fn last_recruiter_turn<C: ConnectionTrait>(
    db: &C,
    keys: &PiiKeys,
    session_id: Uuid,
) -> Result<Option<Turn>, StoreError> {
    let fila = db
        .query_one(stmt(
            "SELECT seq, speaker::text, kind, transcript FROM interview_turns \
             WHERE interview_session_id = $1 AND speaker = 'recruiter' \
             ORDER BY seq DESC LIMIT 1",
            vec![session_id.into()],
        ))
        .await?;
    fila.map(|r| turn_from_row(keys, &r)).transpose()
}

// ------------------------------------------------------------------ answering

/// Most turns (both speakers) a session may hold. A safety net: the engine ends an interview long
/// before this. When there is no room for another full exchange, the session is closed.
pub const MAX_TURNS: i32 = 40;

/// Most answers one person may send in [`TURN_WINDOW_SECS`] seconds, across all their sessions.
pub const MAX_ANSWERS_PER_WINDOW: i64 = 20;
pub const TURN_WINDOW_SECS: i64 = 60;

/// What [`answer_turn`] did. Every variant except `Replied` left the database untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerOutcome {
    /// Both turns were saved. `done`: the session is now `completed`.
    Replied { turn: Turn, done: bool },
    /// Does not exist or is somebody else's.
    NotFound,
    /// Exists, but is not `in_progress`.
    InvalidState,
    /// Another turn of this session is in flight (its row lock is held).
    Busy,
    /// The person already sent [`MAX_ANSWERS_PER_WINDOW`] answers in the last minute.
    RateLimited { retry_after_secs: u64 },
    /// The engine failed (or the recruiter or level is unknown to it): nothing was saved.
    EngineFailed,
}

/// `55P03` (`lock_not_available`): what `FOR UPDATE NOWAIT` raises when the row is locked.
fn is_lock_busy(e: &DbErr) -> bool {
    let (DbErr::Query(RuntimeErr::SqlxError(inner)) | DbErr::Exec(RuntimeErr::SqlxError(inner))) =
        e
    else {
        return false;
    };
    matches!(inner, sea_orm::sqlx::Error::Database(d) if d.code().as_deref() == Some("55P03"))
}

/// The person's side of a transcript as the engine wants it.
fn to_messages(turnos: &[Turn]) -> Vec<Message> {
    turnos
        .iter()
        .map(|t| Message {
            speaker: if t.speaker == "recruiter" {
                Speaker::Recruiter
            } else {
                Speaker::Candidate
            },
            text: t.text.clone(),
        })
        .collect()
}

/// One criterion of the score breakdown, as the contract shows it.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BreakdownItem {
    pub criterion: String,
    pub label: String,
    pub score: i32,
    pub max_points: i32,
    pub why: String,
}

/// What is persisted in `interview_scores.breakdown`: the per-criterion results and the
/// improvements that go with them (the table has no other column for them).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct StoredBreakdown {
    criteria: Vec<BreakdownItem>,
    improvements: Vec<StoredImprovement>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct StoredImprovement {
    criterion: String,
    text: String,
    potential_points: i32,
}

/// `Score` of the contract (`finish` and `GET .../score`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Score {
    pub score: i32,
    pub clarity: i32,
    /// Always `null` in text mode.
    pub confidence: Option<i32>,
    pub breakdown: Vec<BreakdownItem>,
    pub improvements: Vec<String>,
    pub rubric_version: i32,
    pub computed_at: String,
}

/// The criteria of a pinned rubric version, from its `jsonb`.
fn parse_criteria(json: &str) -> Result<Vec<RubricCriterion>, StoreError> {
    #[derive(serde::Deserialize)]
    struct Raw {
        code: String,
        name: String,
        max_points: u32,
        #[serde(default)]
        description: String,
    }
    let raw: Vec<Raw> = serde_json::from_str(json).map_err(|_| StoreError::Corrupt)?;
    Ok(raw
        .into_iter()
        .map(|c| RubricCriterion {
            code: c.code,
            name: c.name,
            max_points: c.max_points,
            description: c.description,
        })
        .collect())
}

/// Runs **inside the transaction** that closes a session, after the closing is written and the
/// status is already `completed`, and before the commit: the score and the level progress are
/// all-or-nothing with the closing itself, so a completed session always has its score.
///
/// - Evaluates the transcript with the rubric version **pinned by the session** (not the current
///   one) through the engine, and stores the result in `interview_scores` (one row per session: the
///   unique index plus `ON CONFLICT DO NOTHING` make a repeated call a no-op, and then the progress
///   is not touched either). `confidence` stays `NULL`.
/// - Upserts the progress of the session's level: `best_score` only goes up and
///   `sessions_completed` grows by one (this function runs once per completed session).
/// - Unlocks exactly the next level (the one whose `previous_level_id` is this level) when it is
///   `available` and the score reaches its `min_score_to_unlock`. Never two levels at once.
pub async fn on_session_completed<C: ConnectionTrait>(
    db: &C,
    keys: &PiiKeys,
    engine: &(dyn InterviewEngine + Send + Sync),
    applicant: Uuid,
    session_id: Uuid,
) -> Result<(), StoreError> {
    let c = |_| StoreError::Corrupt;
    let fila = db
        .query_one(stmt(
            "SELECT r.code, l.code, l.id, v.id, v.criteria::text \
             FROM interview_sessions s \
             JOIN ai_recruiters r ON r.id = s.ai_recruiter_id \
             JOIN interview_levels l ON l.id = s.interview_level_id \
             JOIN rubric_versions v ON v.id = s.rubric_version_id \
             WHERE s.id = $1 AND s.applicant_id = $2",
            vec![session_id.into(), applicant.into()],
        ))
        .await?
        .ok_or(StoreError::Corrupt)?;
    let recruiter_code: String = fila.try_get_by_index(0).map_err(c)?;
    let level_code: String = fila.try_get_by_index(1).map_err(c)?;
    let level_id: Uuid = fila.try_get_by_index(2).map_err(c)?;
    let version_id: Uuid = fila.try_get_by_index(3).map_err(c)?;
    let criteria = parse_criteria(&fila.try_get_by_index::<String>(4).map_err(c)?)?;

    let (Some(recruiter), Some(level)) = (
        RecruiterCode::from_code(&recruiter_code),
        Level::from_code(&level_code),
    ) else {
        return Err(StoreError::Engine);
    };
    let turnos = turns_of(db, keys, session_id).await?;
    let evaluacion = engine
        .evaluate(
            &RecruiterConfig { recruiter, level },
            &to_messages(&turnos),
            &criteria,
        )
        .map_err(|_| StoreError::Engine)?;

    let suma: u32 = evaluacion.breakdown.iter().map(|b| b.points).sum();
    if evaluacion.score > 100 || evaluacion.clarity > 100 || suma != evaluacion.score {
        return Err(StoreError::Engine);
    }
    let almacenado = StoredBreakdown {
        criteria: evaluacion
            .breakdown
            .iter()
            .map(|b| BreakdownItem {
                criterion: b.code.clone(),
                label: b.name.clone(),
                score: b.points as i32,
                max_points: b.max_points as i32,
                why: b.why.clone(),
            })
            .collect(),
        improvements: evaluacion
            .improvements
            .iter()
            .map(|i| StoredImprovement {
                criterion: i.criterion_code.clone(),
                text: i.text.clone(),
                potential_points: i.potential_points as i32,
            })
            .collect(),
    };
    let breakdown = serde_json::to_string(&almacenado).map_err(|_| StoreError::Corrupt)?;
    let score = evaluacion.score as i16;

    let insertadas = db
        .execute(stmt(
            "INSERT INTO interview_scores \
                (id, interview_session_id, score, confidence, clarity, breakdown, rubric_version_id) \
             VALUES ($1, $2, $3, NULL, $4, $5::jsonb, $6) \
             ON CONFLICT (interview_session_id) DO NOTHING",
            vec![
                migration::new_id().into(),
                session_id.into(),
                score.into(),
                (evaluacion.clarity as i16).into(),
                breakdown.into(),
                version_id.into(),
            ],
        ))
        .await?
        .rows_affected();
    if insertadas == 0 {
        // Already scored (and counted) by an earlier call.
        return Ok(());
    }

    db.execute(stmt(
        "INSERT INTO applicant_level_progress \
            (id, applicant_id, interview_level_id, unlocked_at, best_score, sessions_completed) \
         VALUES ($1, $2, $3, now(), $4, 1) \
         ON CONFLICT (applicant_id, interview_level_id) DO UPDATE SET \
            best_score = GREATEST(COALESCE(applicant_level_progress.best_score, 0), \
                                  EXCLUDED.best_score), \
            sessions_completed = applicant_level_progress.sessions_completed + 1",
        vec![
            migration::new_id().into(),
            applicant.into(),
            level_id.into(),
            score.into(),
        ],
    ))
    .await?;
    db.execute(stmt(
        "INSERT INTO applicant_level_progress \
            (id, applicant_id, interview_level_id, unlocked_at, best_score, sessions_completed) \
         SELECT $1, $2, n.id, now(), NULL, 0 FROM interview_levels n \
         WHERE n.previous_level_id = $3 AND n.available AND n.active \
           AND n.min_score_to_unlock IS NOT NULL AND $4 >= n.min_score_to_unlock \
         ON CONFLICT (applicant_id, interview_level_id) DO NOTHING",
        vec![
            migration::new_id().into(),
            applicant.into(),
            level_id.into(),
            score.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The stored score of a session, or `None` if it has none.
async fn stored_score<C: ConnectionTrait>(
    db: &C,
    session_id: Uuid,
) -> Result<Option<Score>, StoreError> {
    let sql = format!(
        "SELECT sc.score::int4, sc.clarity::int4, sc.confidence::int4, sc.breakdown::text, \
                v.version, {} \
         FROM interview_scores sc JOIN rubric_versions v ON v.id = sc.rubric_version_id \
         WHERE sc.interview_session_id = $1",
        iso_sql("sc.computed_at")
    );
    let Some(f) = db.query_one(stmt(&sql, vec![session_id.into()])).await? else {
        return Ok(None);
    };
    let c = |_| StoreError::Corrupt;
    let guardado: StoredBreakdown =
        serde_json::from_str(&f.try_get_by_index::<String>(3).map_err(c)?)
            .map_err(|_| StoreError::Corrupt)?;
    Ok(Some(Score {
        score: f.try_get_by_index(0).map_err(c)?,
        clarity: f.try_get_by_index(1).map_err(c)?,
        confidence: f.try_get_by_index(2).map_err(c)?,
        breakdown: guardado.criteria,
        improvements: guardado.improvements.into_iter().map(|i| i.text).collect(),
        rubric_version: f.try_get_by_index(4).map_err(c)?,
        computed_at: f.try_get_by_index(5).map_err(c)?,
    }))
}

/// What [`score_of`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScoreOutcome {
    Found(Score),
    /// Does not exist or is somebody else's.
    NotFound,
    /// Exists, but is not `completed`.
    InvalidState,
}

/// The score of a completed session of the person.
pub async fn score_of<C: ConnectionTrait>(
    db: &C,
    applicant: Uuid,
    id: Uuid,
) -> Result<ScoreOutcome, StoreError> {
    let Some(fila) = db
        .query_one(stmt(
            "SELECT status::text FROM interview_sessions \
             WHERE id = $1 AND applicant_id = $2 AND status <> 'created'",
            vec![id.into(), applicant.into()],
        ))
        .await?
    else {
        return Ok(ScoreOutcome::NotFound);
    };
    let status: String = fila.try_get_by_index(0).map_err(|_| StoreError::Corrupt)?;
    if status != "completed" {
        return Ok(ScoreOutcome::InvalidState);
    }
    // A completed session always has its score: closing and scoring are one transaction.
    stored_score(db, id)
        .await?
        .map(ScoreOutcome::Found)
        .ok_or(StoreError::Corrupt)
}

/// Save the person's answer and the recruiter's reply, or neither.
///
/// One transaction: the session row is locked with `FOR UPDATE NOWAIT`, so a second simultaneous
/// turn fails fast with [`AnswerOutcome::Busy`] instead of waiting, and `seq` is computed under the
/// lock so it can never repeat. The engine is stateless: it is handed the whole history plus the new
/// answer. If it fails the transaction is dropped and the answer is not kept.
///
/// `text` is already trimmed and validated by the caller.
pub async fn answer_turn(
    db: &DatabaseConnection,
    keys: &PiiKeys,
    engine: &(dyn InterviewEngine + Send + Sync),
    applicant: Uuid,
    id: Uuid,
    text: &str,
) -> Result<AnswerOutcome, StoreError> {
    let txn = db.begin().await?;
    let fila = match txn
        .query_one(stmt(
            "SELECT s.status::text, r.code, l.code \
             FROM interview_sessions s \
             JOIN ai_recruiters r ON r.id = s.ai_recruiter_id \
             JOIN interview_levels l ON l.id = s.interview_level_id \
             WHERE s.id = $1 AND s.applicant_id = $2 AND s.status <> 'created' \
             FOR UPDATE OF s NOWAIT",
            vec![id.into(), applicant.into()],
        ))
        .await
    {
        Ok(f) => f,
        Err(e) if is_lock_busy(&e) => {
            let _ = txn.rollback().await;
            return Ok(AnswerOutcome::Busy);
        }
        Err(_) => return Err(StoreError::Db),
    };
    let Some(fila) = fila else {
        let _ = txn.rollback().await;
        return Ok(AnswerOutcome::NotFound);
    };
    let c = |_| StoreError::Corrupt;
    let status: String = fila.try_get_by_index(0).map_err(c)?;
    let recruiter_code: String = fila.try_get_by_index(1).map_err(c)?;
    let level_code: String = fila.try_get_by_index(2).map_err(c)?;
    if status != "in_progress" {
        let _ = txn.rollback().await;
        return Ok(AnswerOutcome::InvalidState);
    }

    // The window is per person (all their sessions), and the lock makes the count exact: a person
    // has one open session, and its row is ours.
    let ventana = txn
        .query_one(stmt(
            "SELECT count(*)::bigint, \
                    COALESCE(GREATEST(1, CEIL($2::double precision \
                        - extract(epoch from (now() - min(t.created_at)))))::bigint, 1) \
             FROM interview_turns t \
             JOIN interview_sessions s ON s.id = t.interview_session_id \
             WHERE s.applicant_id = $1 AND t.speaker = 'applicant' \
               AND t.created_at > now() - make_interval(secs => $2::double precision)",
            vec![applicant.into(), (TURN_WINDOW_SECS as f64).into()],
        ))
        .await?
        .ok_or(StoreError::Corrupt)?;
    let recientes: i64 = ventana.try_get_by_index(0).map_err(c)?;
    if recientes >= MAX_ANSWERS_PER_WINDOW {
        let espera: i64 = ventana.try_get_by_index(1).map_err(c)?;
        let _ = txn.rollback().await;
        return Ok(AnswerOutcome::RateLimited {
            retry_after_secs: espera.clamp(1, TURN_WINDOW_SECS) as u64,
        });
    }

    let turnos = turns_of(&txn, keys, id).await?;
    let ultimo = turnos.last().map_or(0, |t| t.seq);
    let (seq_persona, seq_recruiter) = (ultimo + 1, ultimo + 2);

    let (Some(recruiter), Some(level)) = (
        RecruiterCode::from_code(&recruiter_code),
        Level::from_code(&level_code),
    ) else {
        let _ = txn.rollback().await;
        return Ok(AnswerOutcome::EngineFailed);
    };
    let config = RecruiterConfig { recruiter, level };
    let mut historial = to_messages(&turnos);
    historial.push(Message::candidate(text));
    let Ok(siguiente) = engine.next_turn(&config, &historial) else {
        let _ = txn.rollback().await;
        return Ok(AnswerOutcome::EngineFailed);
    };

    // Closed by the engine, or by the cap: no room left for another full exchange.
    let cierra = siguiente.done || seq_recruiter + 2 > MAX_TURNS;
    let (kind, mensaje) = if cierra {
        let mensaje = if siguiente.done {
            siguiente.message
        } else {
            closing_message(recruiter).to_string()
        };
        ("closing", mensaje)
    } else {
        (persisted_kind(siguiente.kind), siguiente.message)
    };

    insert_turn(
        &txn,
        keys,
        id,
        seq_persona,
        Speaker::Candidate,
        "answer",
        text,
    )
    .await?;
    insert_turn(
        &txn,
        keys,
        id,
        seq_recruiter,
        Speaker::Recruiter,
        kind,
        &mensaje,
    )
    .await?;
    let sql = if cierra {
        "UPDATE interview_sessions SET status = 'completed', completed_at = now(), \
                last_activity_at = now(), updated_at = now() WHERE id = $1"
    } else {
        "UPDATE interview_sessions SET last_activity_at = now(), updated_at = now() WHERE id = $1"
    };
    txn.execute(stmt(sql, vec![id.into()])).await?;
    if cierra {
        match on_session_completed(&txn, keys, engine, applicant, id).await {
            Ok(()) => {}
            Err(StoreError::Engine) => {
                let _ = txn.rollback().await;
                return Ok(AnswerOutcome::EngineFailed);
            }
            Err(e) => return Err(e),
        }
    }
    txn.commit().await?;
    Ok(AnswerOutcome::Replied {
        turn: Turn {
            seq: seq_recruiter,
            speaker: "recruiter".to_string(),
            kind: kind.to_string(),
            text: mensaje,
        },
        done: cierra,
    })
}

// ------------------------------------------------------------------ finishing

/// Fewest answers of the person a session needs to be finished by hand.
pub const MIN_ANSWERS_TO_FINISH: i64 = 2;

/// What [`finish_session`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishOutcome {
    /// Completed (now, or already was): the score.
    Finished(Score),
    NotFound,
    /// `paused` or `abandoned`.
    InvalidState,
    /// Another request holds the session row (a turn or another finish in flight).
    Busy,
    /// Fewer than [`MIN_ANSWERS_TO_FINISH`] answers: the session stays open.
    TooShort,
    /// The engine failed: nothing was changed.
    EngineFailed,
}

/// Finish a session: close it and score it, or hand back the score it already has.
///
/// Same lock as [`answer_turn`] (`FOR UPDATE NOWAIT` on the session row), so a finish racing a turn
/// or another finish gets [`FinishOutcome::Busy`] instead of waiting. Under the lock the status is
/// read once: `completed` returns the stored score without evaluating again (idempotent),
/// `in_progress` is closed through [`on_session_completed`], anything else is an invalid state.
pub async fn finish_session(
    db: &DatabaseConnection,
    keys: &PiiKeys,
    engine: &(dyn InterviewEngine + Send + Sync),
    applicant: Uuid,
    id: Uuid,
) -> Result<FinishOutcome, StoreError> {
    let txn = db.begin().await?;
    let fila = match txn
        .query_one(stmt(
            "SELECT s.status::text FROM interview_sessions s \
             WHERE s.id = $1 AND s.applicant_id = $2 AND s.status <> 'created' \
             FOR UPDATE OF s NOWAIT",
            vec![id.into(), applicant.into()],
        ))
        .await
    {
        Ok(f) => f,
        Err(e) if is_lock_busy(&e) => {
            let _ = txn.rollback().await;
            return Ok(FinishOutcome::Busy);
        }
        Err(_) => return Err(StoreError::Db),
    };
    let Some(fila) = fila else {
        let _ = txn.rollback().await;
        return Ok(FinishOutcome::NotFound);
    };
    let status: String = fila.try_get_by_index(0).map_err(|_| StoreError::Corrupt)?;
    match status.as_str() {
        "completed" => {
            let score = stored_score(&txn, id).await?.ok_or(StoreError::Corrupt)?;
            let _ = txn.rollback().await;
            return Ok(FinishOutcome::Finished(score));
        }
        "in_progress" => {}
        _ => {
            let _ = txn.rollback().await;
            return Ok(FinishOutcome::InvalidState);
        }
    }

    let respuestas: i64 = txn
        .query_one(stmt(
            "SELECT count(*)::bigint FROM interview_turns \
             WHERE interview_session_id = $1 AND speaker = 'applicant'",
            vec![id.into()],
        ))
        .await?
        .ok_or(StoreError::Corrupt)?
        .try_get_by_index(0)
        .map_err(|_| StoreError::Corrupt)?;
    if respuestas < MIN_ANSWERS_TO_FINISH {
        let _ = txn.rollback().await;
        return Ok(FinishOutcome::TooShort);
    }

    txn.execute(stmt(
        "UPDATE interview_sessions SET status = 'completed', completed_at = now(), \
                last_activity_at = now(), updated_at = now() WHERE id = $1",
        vec![id.into()],
    ))
    .await?;
    match on_session_completed(&txn, keys, engine, applicant, id).await {
        Ok(()) => {}
        Err(StoreError::Engine) => {
            let _ = txn.rollback().await;
            return Ok(FinishOutcome::EngineFailed);
        }
        Err(e) => return Err(e),
    }
    let score = stored_score(&txn, id).await?.ok_or(StoreError::Corrupt)?;
    txn.commit().await?;
    Ok(FinishOutcome::Finished(score))
}

// ------------------------------------------------------------------ sessions

/// What [`create_session`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    Created {
        session_id: Uuid,
    },
    /// The person already has an open (in progress or paused) session.
    OpenExists {
        session_id: Uuid,
    },
    /// The monthly quota of the person's plan is used up; `renews_at` is the first instant of the
    /// next UTC month (ISO 8601). Nothing was written.
    LimitReached {
        renews_at: String,
    },
}

/// The user that owns an applicant row.
async fn user_of_applicant<C: ConnectionTrait>(
    db: &C,
    applicant: Uuid,
) -> Result<Uuid, StoreError> {
    let row = db
        .query_one(stmt(
            "SELECT user_id FROM applicants WHERE id = $1",
            vec![applicant.into()],
        ))
        .await?
        .ok_or(StoreError::Corrupt)?;
    Ok(row.try_get_by_index(0)?)
}

/// The id of the person's open session (`in_progress` or `paused`), if any.
pub async fn open_session_id<C: ConnectionTrait>(
    db: &C,
    applicant: Uuid,
) -> Result<Option<Uuid>, StoreError> {
    let row = db
        .query_one(stmt(
            "SELECT id FROM interview_sessions \
             WHERE applicant_id = $1 AND status IN ('in_progress', 'paused')",
            vec![applicant.into()],
        ))
        .await?;
    Ok(row.map(|r| r.try_get_by_index(0)).transpose()?)
}

/// Start a session directly `in_progress`, pinning the **current** rubric version of the
/// recruiter, together with its first turn (kind `question`), in one transaction.
///
/// Two simultaneous calls for the same person cannot both win: the partial unique index
/// `interview_sessions_one_open_per_applicant_idx` rejects the second insert, and that violation is
/// translated to [`CreateOutcome::OpenExists`] with the id of the winner. Nothing is left behind
/// by the loser (the transaction is rolled back).
pub async fn create_session(
    db: &DatabaseConnection,
    keys: &PiiKeys,
    applicant: Uuid,
    recruiter: Uuid,
    level: Uuid,
    first_question: &str,
    policy: QuotaPolicy,
) -> Result<CreateOutcome, StoreError> {
    // The winner can finish (abandon, delete) between our violation and our look-up: retry.
    for _ in 0..3 {
        let txn = db.begin().await?;
        // Una creación a la vez por persona: la cuota se cuenta y se anota sin carreras.
        let user = user_of_applicant(&txn, applicant).await?;
        quota::lock_user(&txn, user).await?;
        let version = txn
            .query_one(stmt(
                "SELECT v.id FROM ai_recruiters r \
                 JOIN rubric_versions v ON v.rubric_id = r.rubric_id AND v.is_current \
                 WHERE r.id = $1",
                vec![recruiter.into()],
            ))
            .await?
            .ok_or(StoreError::NoRubric)?;
        let version: Uuid = version
            .try_get_by_index(0)
            .map_err(|_| StoreError::Corrupt)?;

        let id = migration::new_id();
        let insertada = txn
            .execute(stmt(
                "INSERT INTO interview_sessions \
                    (id, applicant_id, ai_recruiter_id, interview_level_id, rubric_version_id, \
                     status, started_at, last_activity_at) \
                 VALUES ($1, $2, $3, $4, $5, 'in_progress', now(), now())",
                vec![
                    id.into(),
                    applicant.into(),
                    recruiter.into(),
                    level.into(),
                    version.into(),
                ],
            ))
            .await;
        match insertada {
            Ok(_) => {}
            Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
                let _ = txn.rollback().await;
                if let Some(session_id) = open_session_id(db, applicant).await? {
                    return Ok(CreateOutcome::OpenExists { session_id });
                }
                continue;
            }
            Err(_) => return Err(StoreError::Db),
        }
        // Después del índice (la sesión abierta gana a la cuota, como dice el contrato) y antes
        // de confirmar: si no cabe, se deshace todo, sesión incluida.
        if policy.enforce {
            if let Some(renews_at) = quota::exceeded(&txn, user, policy.now_micros).await? {
                let _ = txn.rollback().await;
                return Ok(CreateOutcome::LimitReached { renews_at });
            }
        }
        quota::record_session(&txn, user, id, policy.now_micros).await?;
        insert_turn(
            &txn,
            keys,
            id,
            1,
            Speaker::Recruiter,
            "question",
            first_question,
        )
        .await?;
        txn.commit().await?;
        return Ok(CreateOutcome::Created { session_id: id });
    }
    Err(StoreError::Db)
}

/// One session of the person with all its turns decrypted, or `None` if it does not exist **or
/// is somebody else's** (the two are the same answer on purpose).
pub async fn get_session<C: ConnectionTrait>(
    db: &C,
    keys: &PiiKeys,
    applicant: Uuid,
    id: Uuid,
) -> Result<Option<Interview>, StoreError> {
    let sql = format!(
        "SELECT s.id::text, s.status::text, r.id::text, r.display_name, r.role_title, \
                l.id::text, l.code, l.display_name, {} \
         FROM interview_sessions s \
         JOIN ai_recruiters r ON r.id = s.ai_recruiter_id \
         JOIN interview_levels l ON l.id = s.interview_level_id \
         WHERE s.id = $1 AND s.applicant_id = $2 AND s.status <> 'created'",
        iso_sql("s.created_at")
    );
    let Some(f) = db
        .query_one(stmt(&sql, vec![id.into(), applicant.into()]))
        .await?
    else {
        return Ok(None);
    };
    let c = |_| StoreError::Corrupt;
    let turns = turns_of(db, keys, id).await?;
    Ok(Some(Interview {
        id: f.try_get_by_index(0).map_err(c)?,
        status: f.try_get_by_index(1).map_err(c)?,
        recruiter: RecruiterRef {
            id: f.try_get_by_index(2).map_err(c)?,
            display_name: f.try_get_by_index(3).map_err(c)?,
            role_title: f.try_get_by_index(4).map_err(c)?,
        },
        level: LevelRef {
            id: f.try_get_by_index(5).map_err(c)?,
            code: f.try_get_by_index(6).map_err(c)?,
            name: f.try_get_by_index(7).map_err(c)?,
        },
        created_at: f.try_get_by_index(8).map_err(c)?,
        turns,
    }))
}

/// Position in the listing: `created_at` (microseconds since the epoch) and `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub created_micros: i64,
    pub id: Uuid,
}

/// Latest `created_at` a cursor may carry (year 9999): keeps the SQL arithmetic in range.
const MAX_MICROS: i64 = 253_402_300_799_000_000;

impl Cursor {
    /// Opaque text for the client.
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(format!("{}.{}", self.created_micros, self.id))
    }

    /// `None` for anything that is not exactly what [`Cursor::encode`] produces.
    pub fn decode(text: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
        let texto = String::from_utf8(bytes).ok()?;
        let (micros, id) = texto.split_once('.')?;
        let created_micros: i64 = micros.parse().ok()?;
        if !(0..=MAX_MICROS).contains(&created_micros) {
            return None;
        }
        let id = Uuid::parse_str(id).ok()?;
        Some(Self { created_micros, id })
    }
}

/// A page of the person's sessions, newest first, without transcripts. `limit` rows at most plus
/// the cursor of the next page (`None` on the last one). `status` is already validated.
pub async fn list_sessions<C: ConnectionTrait>(
    db: &C,
    applicant: Uuid,
    status: Option<&str>,
    limit: u64,
    after: Option<Cursor>,
) -> Result<(Vec<Summary>, Option<Cursor>), StoreError> {
    let sql = format!(
        "SELECT s.id::text, s.status::text, l.id::text, l.code, l.display_name, \
                r.id::text, r.display_name, sc.score::int4, {}, {}, \
                (extract(epoch from s.created_at) * 1000000)::bigint, s.id \
         FROM interview_sessions s \
         JOIN ai_recruiters r ON r.id = s.ai_recruiter_id \
         JOIN interview_levels l ON l.id = s.interview_level_id \
         LEFT JOIN interview_scores sc ON sc.interview_session_id = s.id \
         WHERE s.applicant_id = $1 AND s.status <> 'created' \
           AND ($2::text IS NULL OR s.status::text = $2) \
           AND ($3::bigint IS NULL OR (s.created_at, s.id) < \
                (timestamptz 'epoch' + $3::bigint * interval '1 microsecond', $4::uuid)) \
         ORDER BY s.created_at DESC, s.id DESC \
         LIMIT $5",
        iso_sql("s.created_at"),
        iso_sql("s.completed_at"),
    );
    let filas = db
        .query_all(stmt(
            &sql,
            vec![
                applicant.into(),
                status.map(str::to_string).into(),
                after.map(|c| c.created_micros).into(),
                after.map(|c| c.id).into(),
                ((limit + 1) as i64).into(),
            ],
        ))
        .await?;
    let c = |_| StoreError::Corrupt;
    let mut items = Vec::new();
    let mut ultimo = None;
    for (i, f) in filas.iter().enumerate() {
        if i as u64 == limit {
            // The extra row only proves there is a next page.
            return Ok((items, ultimo));
        }
        items.push(Summary {
            id: f.try_get_by_index(0).map_err(c)?,
            status: f.try_get_by_index(1).map_err(c)?,
            level: LevelRef {
                id: f.try_get_by_index(2).map_err(c)?,
                code: f.try_get_by_index(3).map_err(c)?,
                name: f.try_get_by_index(4).map_err(c)?,
            },
            recruiter: SummaryRecruiter {
                id: f.try_get_by_index(5).map_err(c)?,
                display_name: f.try_get_by_index(6).map_err(c)?,
            },
            score: f.try_get_by_index(7).map_err(c)?,
            created_at: f.try_get_by_index(8).map_err(c)?,
            completed_at: f.try_get_by_index(9).map_err(c)?,
        });
        ultimo = Some(Cursor {
            created_micros: f.try_get_by_index(10).map_err(c)?,
            id: f.try_get_by_index(11).map_err(c)?,
        });
    }
    Ok((items, None))
}

/// The state changes the API allows on a session of the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// `in_progress` -> `paused`.
    Pause,
    /// `paused` -> `in_progress`.
    Resume,
    /// `in_progress` or `paused` -> `abandoned`.
    Abandon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionOutcome {
    Done,
    /// Does not exist or is somebody else's.
    NotFound,
    /// Exists, but not in a state this change is allowed from. Nothing was touched.
    InvalidState,
}

/// Apply a change atomically: the status check and the write are one `UPDATE`, so two requests
/// cannot both succeed from the same state.
pub async fn transition<C: ConnectionTrait>(
    db: &C,
    applicant: Uuid,
    id: Uuid,
    change: Transition,
) -> Result<TransitionOutcome, StoreError> {
    let sql = match change {
        Transition::Pause => {
            "UPDATE interview_sessions SET status = 'paused', paused_at = now(), \
                    last_activity_at = now(), updated_at = now() \
             WHERE id = $1 AND applicant_id = $2 AND status = 'in_progress'"
        }
        Transition::Resume => {
            "UPDATE interview_sessions SET status = 'in_progress', paused_at = NULL, \
                    last_activity_at = now(), updated_at = now() \
             WHERE id = $1 AND applicant_id = $2 AND status = 'paused'"
        }
        Transition::Abandon => {
            "UPDATE interview_sessions SET status = 'abandoned', updated_at = now() \
             WHERE id = $1 AND applicant_id = $2 AND status IN ('in_progress', 'paused')"
        }
    };
    let hechas = db
        .execute(stmt(sql, vec![id.into(), applicant.into()]))
        .await?
        .rows_affected();
    if hechas > 0 {
        return Ok(TransitionOutcome::Done);
    }
    let existe = db
        .query_one(stmt(
            "SELECT 1 FROM interview_sessions \
             WHERE id = $1 AND applicant_id = $2 AND status <> 'created'",
            vec![id.into(), applicant.into()],
        ))
        .await?
        .is_some();
    Ok(if existe {
        TransitionOutcome::InvalidState
    } else {
        TransitionOutcome::NotFound
    })
}

/// What [`resume_session`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Resumed: the last thing the recruiter said, so the person can continue from there.
    Resumed(Turn),
    NotFound,
    InvalidState,
}

/// `paused` -> `in_progress` and the last recruiter turn, in one transaction. Writes no turn: it
/// neither repeats nor skips a question, the open one is simply handed back.
pub async fn resume_session(
    db: &DatabaseConnection,
    keys: &PiiKeys,
    applicant: Uuid,
    id: Uuid,
) -> Result<ResumeOutcome, StoreError> {
    let txn = db.begin().await?;
    match transition(&txn, applicant, id, Transition::Resume).await? {
        TransitionOutcome::NotFound => Ok(ResumeOutcome::NotFound),
        TransitionOutcome::InvalidState => Ok(ResumeOutcome::InvalidState),
        TransitionOutcome::Done => {
            let turno = last_recruiter_turn(&txn, keys, id)
                .await?
                .ok_or(StoreError::Corrupt)?;
            txn.commit().await?;
            Ok(ResumeOutcome::Resumed(turno))
        }
    }
}

/// Delete a session of the person (any state). Turns and score go with it (`ON DELETE CASCADE`);
/// the accumulated level progress is a different table and is not touched. `false` if it does not
/// exist or is somebody else's.
pub async fn delete_session<C: ConnectionTrait>(
    db: &C,
    applicant: Uuid,
    id: Uuid,
) -> Result<bool, StoreError> {
    let hechas = db
        .execute(stmt(
            "DELETE FROM interview_sessions \
             WHERE id = $1 AND applicant_id = $2 AND status <> 'created'",
            vec![id.into(), applicant.into()],
        ))
        .await?
        .rows_affected();
    Ok(hechas > 0)
}

/// Move to `abandoned` every session paused for more than `max_paused` as of `now`, which frees
/// the person's open slot. The clock and the threshold are parameters so a test does not have to
/// wait (the task itself passes `SystemTime::now()` and [`PAUSED_MAX_AGE`]). Returns how many
/// sessions were moved. Idempotent.
pub async fn abandon_stale_paused<C: ConnectionTrait>(
    db: &C,
    now: SystemTime,
    max_paused: Duration,
) -> Result<u64, StoreError> {
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let cutoff = since_epoch.saturating_sub(max_paused).as_micros();
    let cutoff = i64::try_from(cutoff).unwrap_or(i64::MAX);
    let hechas = db
        .execute(stmt(
            "UPDATE interview_sessions SET status = 'abandoned', updated_at = now() \
             WHERE status = 'paused' \
               AND COALESCE(paused_at, last_activity_at) \
                   < timestamptz 'epoch' + $1::bigint * interval '1 microsecond'",
            vec![cutoff.into()],
        ))
        .await?
        .rows_affected();
    Ok(hechas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_reprompt_is_persisted_as_a_question() {
        assert_eq!(persisted_kind(TurnKind::Reprompt), "question");
        assert_eq!(persisted_kind(TurnKind::Question), "question");
        assert_eq!(persisted_kind(TurnKind::FollowUp), "follow_up");
        assert_eq!(persisted_kind(TurnKind::Closing), "closing");
    }

    #[test]
    fn cursor_round_trips() {
        let c = Cursor {
            created_micros: 1_762_000_000_123_456,
            id: Uuid::now_v7(),
        };
        assert_eq!(Cursor::decode(&c.encode()), Some(c));
    }

    #[test]
    fn cursor_rejects_anything_else() {
        let valid = URL_SAFE_NO_PAD.encode(format!("5.{}", Uuid::now_v7()));
        assert!(Cursor::decode(&valid).is_some());
        for bad in [
            "",
            "!!!",
            "MDowOjA",
            &URL_SAFE_NO_PAD.encode("5"),
            &URL_SAFE_NO_PAD.encode("-1.00000000-0000-0000-0000-000000000000"),
            &URL_SAFE_NO_PAD.encode("x.00000000-0000-0000-0000-000000000000"),
            &URL_SAFE_NO_PAD.encode("5.no-uuid"),
            &URL_SAFE_NO_PAD.encode(format!("{}.{}", i64::MAX, Uuid::now_v7())),
        ] {
            assert_eq!(Cursor::decode(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn speakers_map_to_the_enum_labels() {
        assert_eq!(speaker_sql(Speaker::Recruiter), "recruiter");
        assert_eq!(speaker_sql(Speaker::Candidate), "applicant");
    }
}
