use std::sync::Arc;

use joker::{
    Agent, Conversation, FixedWindowContextBuilder, Message, RunRequest, ScriptedModel,
    ScriptedStep,
};

#[tokio::test]
async fn fixed_window_trims_model_visible_context_not_canonical_transcript() {
    let mut conversation = Conversation::new();
    conversation.push(Message::user("first"));
    conversation.push(Message::assistant(vec![joker::Content::text("second")]));
    conversation.push(Message::user("third"));

    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::text("fourth")])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_context_builder(Arc::new(FixedWindowContextBuilder::new(1)));

    let outcome = agent
        .run(RunRequest::with_conversation(conversation))
        .await
        .unwrap();

    assert_eq!(outcome.conversation.messages().len(), 4);
    assert_eq!(outcome.conversation.messages()[0], Message::user("first"));
    assert_eq!(
        outcome.conversation.messages()[3].content,
        vec![joker::Content::text("fourth")]
    );
}
