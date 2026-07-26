use criterion::{Criterion, criterion_group, criterion_main};
use joker::{
    ContextBuilder, ContextInput, ContextLimits, Conversation, FixedWindowContextBuilder,
    Message, estimate_tokens, micro_dedup_messages,
};
use tokio::runtime::Runtime;

fn make_messages(count: usize) -> Vec<Message> {
    let mut conv = Conversation::new();
    for i in 0..count {
        let msg = if i % 2 == 0 {
            Message::user(format!("message {i} with some content"))
        } else {
            Message::assistant(vec![joker::Content::text(format!(
                "response {i} with some content"
            ))])
        };
        conv.push(msg);
    }
    conv.into_messages()
}

fn bench_estimate_tokens(c: &mut Criterion) {
    let msgs = make_messages(20);

    c.benchmark_group("context_builder")
        .bench_function("estimate_tokens_20_messages", |b| {
            b.iter(|| {
                let _ = estimate_tokens(&msgs);
            })
        });
}

fn bench_micro_dedup(c: &mut Criterion) {
    let mut msgs = make_messages(50);

    c.benchmark_group("context_builder")
        .bench_function("micro_dedup_50_messages", |b| {
            b.iter(|| {
                let _ = micro_dedup_messages(&mut msgs);
            })
        });
}

fn bench_build_context_small(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let conv = Conversation::from_messages(make_messages(10));
    let builder = FixedWindowContextBuilder::new(64);
    let input = ContextInput {
        conversation: &conv,
        limits: ContextLimits::default(),
    };

    c.benchmark_group("context_builder")
        .bench_function("build_10_messages", |b| {
            b.to_async(&rt).iter(|| async {
                let _ = builder.build(input).await.unwrap();
            })
        });
}

fn bench_build_context_large(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let conv = Conversation::from_messages(make_messages(100));
    let builder = FixedWindowContextBuilder::new(64);
    let input = ContextInput {
        conversation: &conv,
        limits: ContextLimits::default(),
    };

    c.benchmark_group("context_builder")
        .bench_function("build_100_messages", |b| {
            b.to_async(&rt).iter(|| async {
                let _ = builder.build(input).await.unwrap();
            })
        });
}

criterion_group!(
    name = context_builder;
    config = Criterion::default().sample_size(50);
    targets = bench_estimate_tokens, bench_micro_dedup, bench_build_context_small,
             bench_build_context_large
);
criterion_main!(context_builder);
