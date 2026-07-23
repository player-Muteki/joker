use std::sync::Arc;

use joker::{
    Agent, Conversation, FixedWindowContextBuilder, Message, RunRequest, ScriptedModel,
    ScriptedStep,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut conversation = Conversation::new();
    conversation.push(Message::user("older message"));
    conversation.push(Message::user("visible message"));

    let model = Arc::new(ScriptedModel::new([ScriptedStep::text(
        "trimmed context was accepted",
    )])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_context_builder(Arc::new(FixedWindowContextBuilder::new(1)));

    let outcome = agent
        .run(RunRequest::with_conversation(conversation))
        .await
        .unwrap();
    println!(
        "canonical transcript length: {}",
        outcome.conversation.messages().len()
    );
}
