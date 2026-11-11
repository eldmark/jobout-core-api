//! Manual live smoke of the AI wording against the real OpenRouter (JB-88). Ignored: CI and the
//! normal suite never run it and never need a key.
//!
//!   OPENROUTER_API_KEY=... cargo test --test interview_ai_replies_live -- --ignored --nocapture
//!
//! It reads the key from the environment only. It prints the model that answered, the latency,
//! whether it fell back, and at most the first 120 characters of the recruiter's reply. Never the key.

use std::time::Instant;

use jobout_core_api::interview::{
    openrouter::{OpenRouterConfig, OpenRouterEngine},
    DeterministicEngine, InterviewEngine, Level, Message, RecruiterCode, RecruiterConfig,
};

fn head(text: &str) -> String {
    text.chars()
        .take(120)
        .collect::<String>()
        .replace('\n', " ")
}

#[test]
#[ignore = "talks to the real OpenRouter; needs OPENROUTER_API_KEY"]
fn live_smoke_one_turn_per_recruiter_kind() {
    // The info/warn logs carry the model and the counter, never content: safe to show.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("jobout_core_api=debug")
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .try_init();
    let env = |name: &str| std::env::var(name).ok();
    let config = OpenRouterConfig::from_lookup(|n| match n {
        "AI_REPLIES_ENABLED" => Some("true".into()),
        other => env(other),
    })
    .expect("set OPENROUTER_API_KEY in the environment");
    println!("models: {:?}  timeout: {:?}", config.models, config.timeout);
    let engine = OpenRouterEngine::new(config);

    let cfg = RecruiterConfig {
        recruiter: RecruiterCode::Ana,
        level: Level::Junior,
    };
    let opening_det = DeterministicEngine.next_turn(&cfg, &[]).unwrap();
    let t0 = Instant::now();
    let opening = engine.next_turn(&cfg, &[]).unwrap();
    println!(
        "opening  latency={:?} fell_back={} text={:?}",
        t0.elapsed(),
        opening.message == opening_det.message,
        head(&opening.message)
    );

    let h = vec![
        Message::recruiter(opening_det.message.clone()),
        Message::candidate("Pues me gusta trabajar en equipo y aprender cosas nuevas."),
    ];
    let t0 = Instant::now();
    let follow = engine.next_turn(&cfg, &h).unwrap();
    println!(
        "follow-up latency={:?} kind={:?} text={:?}",
        t0.elapsed(),
        follow.kind,
        head(&follow.message)
    );

    let sofia = RecruiterConfig {
        recruiter: RecruiterCode::Sofia,
        level: Level::Junior,
    };
    let t0 = Instant::now();
    let en = engine.next_turn(&sofia, &[]).unwrap();
    println!(
        "sofia    latency={:?} text={:?}",
        t0.elapsed(),
        head(&en.message)
    );
    println!("fallbacks_total={}", engine.fallback_count());
}
