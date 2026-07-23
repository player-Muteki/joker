use std::sync::Arc;

use joker::{Agent, RunRequest, ScriptedModel, ScriptedStep};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let model = Arc::new(ScriptedModel::new([ScriptedStep::text("hello from joker")]))
        as Arc<dyn joker::Model>;
    let agent = Agent::new(model);

    let outcome = agent.run(RunRequest::new("hello")).await.unwrap();
    println!("{:#?}", outcome.conversation.messages());
}
