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
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, SqlErr, Statement, TransactionTrait,
    Value,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    crypto::PiiKeys,
    interview::{
        store_crypto::{decrypt_turn_text, encrypt_turn_text},
        Speaker, TurnKind,
    },
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

// ------------------------------------------------------------------ sessions

/// What [`create_session`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateOutcome {
    Created {
        session_id: Uuid,
    },
    /// The person already has an open (in progress or paused) session.
    OpenExists {
        session_id: Uuid,
    },
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
) -> Result<CreateOutcome, StoreError> {
    // The winner can finish (abandon, delete) between our violation and our look-up: retry.
    for _ in 0..3 {
        let txn = db.begin().await?;
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
