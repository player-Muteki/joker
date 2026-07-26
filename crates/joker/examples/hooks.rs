use std::sync::Arc;

use joker::{Conversation, Hook, HookRegistry, ToolInvocation, ToolName, ToolOutput};
use serde_json::json;

#[derive(Clone)]
struct AuditHook(String);

impl Hook for AuditHook {
    fn before_tool_call(
        &self,
        invocation: &ToolInvocation,
        _conversation: &Conversation,
    ) -> Result<(), String> {
        println!("[{}] before: {}", self.0, invocation.name);
        Ok(())
    }

    fn after_tool_call(
        &self,
        _invocation: &ToolInvocation,
        output: &mut ToolOutput,
        _conversation: &mut Conversation,
    ) {
        println!("[{}] after: output={}", self.0, output.output);
    }
}

fn main() {
    let mut registry = HookRegistry::new();
    registry.register(Arc::new(AuditHook("hook1".into())));
    registry.register(Arc::new(AuditHook("hook2".into())));

    let invocation = ToolInvocation {
        name: ToolName::new("echo"),
        arguments: json!({"msg": "hello"}),
        call_id: "c1".into(),
    };
    let mut output = ToolOutput::new(json!("done"));
    let mut conv = Conversation::new();

    registry.before_tool_call(&invocation, &conv).unwrap();
    registry.after_tool_call(&invocation, &mut output, &mut conv);

    println!("\nhook pipeline completed");
    println!("final output: {}", output.output);
}
