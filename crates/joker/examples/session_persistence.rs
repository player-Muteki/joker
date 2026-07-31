use std::sync::Arc;

use joker::{
    Agent, Content, Conversation, JsonlSessionStore, Message, RunRequest, ScriptedModel,
    ScriptedStep, SessionData, SessionStore,
};
use tempfile::TempDir;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let dir = TempDir::new().unwrap();
    let store = JsonlSessionStore::new(dir.path()).expect("create session store");

    let mut conv = Conversation::new();
    conv.push(Message::user("hello"));
    conv.push(Message::assistant(vec![Content::text("hi there")]));

    store
        .save(SessionData {
            id: "session-1".into(),
            label: "chat example".into(),
            created_at: 0,
            updated_at: 0,
            model: "test-model".into(),
            agent_name: "build".into(),
            parent_id: None,
            root_id: "session-1".into(),
            conversation: conv,
        })
        .await
        .expect("save session");

    let loaded = store.load("session-1").await.expect("load session");
    if let Some(session) = loaded {
        println!("loaded {} messages", session.conversation.messages().len());
        let fork = store
            .fork("session-1", "forked".into(), "plan".into(), "gpt-4".into())
            .await
            .expect("fork session")
            .expect("forked session exists");
        println!(
            "forked session: id={}, parent={:?}",
            fork.id, fork.parent_id
        );
    }

    let sessions = store.list().await.expect("list sessions");
    println!("total sessions: {}", sessions.len());
    for info in &sessions {
        println!("  - {} ({})", info.id, info.label);
    }

    if let Some(session) = store.load(&sessions[0].id).await.unwrap() {
        let model = Arc::new(ScriptedModel::new([ScriptedStep::text(
            "loaded from session",
        )])) as Arc<dyn joker::Model>;
        let agent = Agent::new(model);
        let outcome = agent
            .run(RunRequest::with_conversation(session.conversation))
            .await
            .expect("run with session");

        let last = outcome.conversation.messages().last().unwrap();
        println!("agent response: {:?}", last.content);
    }
}
