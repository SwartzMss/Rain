use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use backend::{
    AppState,
    ai_provider::client::{
        ChatCompletionClient, ChatMessage, ChatRequest, ChatResponse, ProviderError,
    },
    config::AppLimits,
    db,
    models::skill_runs::NewSkillRun,
    repositories::skill_runs,
    services::skill_runner::SkillRunner,
};

struct ScriptedClient(Mutex<VecDeque<Result<ChatResponse, ProviderError>>>);

#[async_trait]
impl ChatCompletionClient for ScriptedClient {
    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.0.lock().unwrap().pop_front().unwrap()
    }
}

#[tokio::test]
async fn runner_persists_a_valid_structured_result() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ISSUE','Issue')")
        .execute(&pool)
        .await
        .unwrap();
    let run = skill_runs::create(
        &pool,
        &NewSkillRun {
            user_id: "u".into(),
            issue_code: "ISSUE".into(),
            skill_id: "s".into(),
            skill_version: 1,
            skill_name: "Skill".into(),
            skill_snapshot_markdown: "# Analyze".into(),
        },
    )
    .await
    .unwrap();
    let state = actix_web::web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    let (cancellation, _) = state.skill_runs.register(&run.id);
    let client = Arc::new(ScriptedClient(Mutex::new(VecDeque::from([Ok(ChatResponse {
        message: ChatMessage {
            role: "assistant".into(),
            content: Some(r#"{"summary":"No matching evidence","observations":[],"inferences":[],"missing_context":["No logs"],"evidence":[]}"#.into()),
            tool_calls: vec![], tool_call_id: None, name: None,
        }
    })]))));

    SkillRunner::execute(state, run.id.clone(), client, cancellation).await;

    let stored = skill_runs::find(&pool, &run.id).await.unwrap().unwrap();
    assert_eq!(stored.status, "SUCCEEDED");
    assert!(stored.result_json.unwrap().contains("No matching evidence"));
}
