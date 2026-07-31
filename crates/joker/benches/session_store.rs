use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use joker::{Content, Conversation, JsonlSessionStore, SessionData, SessionStore};
use tempfile::TempDir;
use tokio::runtime::Runtime;

fn make_session(size: usize) -> SessionData {
    let mut conv = Conversation::new();
    for i in 0..size {
        let msg = if i % 2 == 0 {
            joker::Message::user(format!(
                "message {i} content with padding for realistic length"
            ))
        } else {
            joker::Message::assistant(vec![Content::text(format!(
                "response {i} with more realistic content length"
            ))])
        };
        conv.push(msg);
    }
    SessionData {
        id: "bench-session-id".into(),
        label: "bench".into(),
        created_at: 0,
        updated_at: 0,
        model: "gpt-4".into(),
        agent_name: "build".into(),
        parent_id: None,
        root_id: "bench-session-id".into(),
        conversation: conv,
    }
}

fn bench_save_10_messages(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.benchmark_group("session_store")
        .bench_function("save_10_messages", |b| {
            b.to_async(&rt).iter_batched(
                || {
                    let dir = TempDir::new().unwrap();
                    let store = JsonlSessionStore::new(dir.path()).unwrap();
                    let session = make_session(10);
                    (store, session, dir)
                },
                |(store, session, _dir)| async move {
                    let _ = store.save(session).await;
                },
                BatchSize::SmallInput,
            )
        });
}

fn bench_save_100_messages(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.benchmark_group("session_store")
        .bench_function("save_100_messages", |b| {
            b.to_async(&rt).iter_batched(
                || {
                    let dir = TempDir::new().unwrap();
                    let store = JsonlSessionStore::new(dir.path()).unwrap();
                    let session = make_session(100);
                    (store, session, dir)
                },
                |(store, session, _dir)| async move {
                    let _ = store.save(session).await;
                },
                BatchSize::SmallInput,
            )
        });
}

fn bench_save_then_load(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.benchmark_group("session_store")
        .bench_function("save_and_load_50_messages", |b| {
            b.to_async(&rt).iter_batched(
                || {
                    let dir = TempDir::new().unwrap();
                    let store = JsonlSessionStore::new(dir.path()).unwrap();
                    let session = make_session(50);
                    let _ = rt.block_on(store.save(session));
                    dir
                },
                |dir| async move {
                    let store = JsonlSessionStore::new(dir.path()).unwrap();
                    let info = store.list().await.unwrap();
                    if let Some(first) = info.first() {
                        let _ = store.load(&first.id).await.unwrap();
                    }
                },
                BatchSize::SmallInput,
            )
        });
}

fn bench_fork_session(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.benchmark_group("session_store")
        .bench_function("fork_50_message_session", |b| {
            b.to_async(&rt).iter_batched(
                || {
                    let dir = TempDir::new().unwrap();
                    let store = JsonlSessionStore::new(dir.path()).unwrap();
                    let session = make_session(50);
                    let _ = rt.block_on(store.save(session));
                    dir
                },
                |dir| async move {
                    let store = JsonlSessionStore::new(dir.path()).unwrap();
                    let info = store.list().await.unwrap();
                    if let Some(first) = info.first() {
                        let _ = store
                            .fork(&first.id, "fork".into(), "build".into(), "gpt-4".into())
                            .await;
                    }
                },
                BatchSize::SmallInput,
            )
        });
}

criterion_group!(
    name = session_store;
    config = Criterion::default().sample_size(50);
    targets = bench_save_10_messages, bench_save_100_messages, bench_save_then_load,
             bench_fork_session
);
criterion_main!(session_store);
