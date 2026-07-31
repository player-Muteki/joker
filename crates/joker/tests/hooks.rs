use std::sync::Arc;
use std::sync::Mutex;

use joker::{Conversation, Hook, HookRegistry, ToolInvocation, ToolName, ToolOutput};
use serde_json::json;

struct RecordingHook {
    before_calls: Mutex<Vec<String>>,
    after_calls: Mutex<Vec<String>>,
}

impl RecordingHook {
    fn new() -> Self {
        Self {
            before_calls: Mutex::new(Vec::new()),
            after_calls: Mutex::new(Vec::new()),
        }
    }
}

impl Hook for RecordingHook {
    fn before_tool_call(
        &self,
        invocation: &ToolInvocation,
        _conversation: &Conversation,
    ) -> Result<(), String> {
        self.before_calls
            .lock()
            .unwrap()
            .push(invocation.name.to_string());
        Ok(())
    }

    fn after_tool_call(
        &self,
        invocation: &ToolInvocation,
        _output: &mut ToolOutput,
        _conversation: &mut Conversation,
    ) {
        self.after_calls
            .lock()
            .unwrap()
            .push(invocation.name.to_string());
    }
}

struct BlockingHook;

impl Hook for BlockingHook {
    fn before_tool_call(
        &self,
        _invocation: &ToolInvocation,
        _conversation: &Conversation,
    ) -> Result<(), String> {
        Err("blocked by policy".into())
    }
}

struct ModifyingHook;

impl Hook for ModifyingHook {
    fn after_tool_call(
        &self,
        _invocation: &ToolInvocation,
        output: &mut ToolOutput,
        _conversation: &mut Conversation,
    ) {
        output.output = json!("modified");
    }
}

fn make_invocation(name: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: "call-1".into(),
        name: ToolName::new(name),
        arguments: json!({"key": "value"}),
    }
}

#[tokio::test]
async fn registered_hook_fires_on_before_tool_call() {
    let hook = Arc::new(RecordingHook::new());
    let mut registry = HookRegistry::new();
    registry.register(hook.clone());

    let invocation = make_invocation("test_tool");
    let conversation = Conversation::new();
    let result = registry.before_tool_call(&invocation, &conversation);

    assert!(result.is_ok());
    let before = hook.before_calls.lock().unwrap();
    assert_eq!(*before, vec!["test_tool"]);
}

#[tokio::test]
async fn blocking_hook_forbids_tool_call() {
    let mut registry = HookRegistry::new();
    registry.register(Arc::new(BlockingHook));

    let invocation = make_invocation("dangerous_tool");
    let conversation = Conversation::new();
    let result = registry.before_tool_call(&invocation, &conversation);

    assert_eq!(result, Err("blocked by policy".into()));
}

#[tokio::test]
async fn registered_hook_modifies_result_in_after_tool_call() {
    let mut registry = HookRegistry::new();
    registry.register(Arc::new(ModifyingHook));

    let invocation = make_invocation("test_tool");
    let mut output = ToolOutput::new(json!("original"));
    let mut conversation = Conversation::new();
    registry.after_tool_call(&invocation, &mut output, &mut conversation);

    assert_eq!(output.output, json!("modified"));
}

#[tokio::test]
async fn no_hooks_registered_is_noop() {
    let registry = HookRegistry::new();
    let invocation = make_invocation("test_tool");
    let mut output = ToolOutput::new(json!("value"));
    let mut conversation = Conversation::new();
    let empty_messages = &mut Vec::new();

    assert!(
        registry
            .before_tool_call(&invocation, &conversation)
            .is_ok()
    );
    registry.after_tool_call(&invocation, &mut output, &mut conversation);
    registry.before_provider_request(empty_messages);
    registry.on_session_start("agent");
    registry.on_session_end("agent");

    assert_eq!(output.output, json!("value"));
}

#[tokio::test]
async fn multiple_hooks_fire_in_registration_order() {
    let order = Arc::new(Mutex::new(Vec::new()));

    struct OrderHook {
        index: usize,
        order: Arc<Mutex<Vec<usize>>>,
    }

    impl Hook for OrderHook {
        fn before_tool_call(
            &self,
            _invocation: &ToolInvocation,
            _conversation: &Conversation,
        ) -> Result<(), String> {
            self.order.lock().unwrap().push(self.index);
            Ok(())
        }

        fn after_tool_call(
            &self,
            _invocation: &ToolInvocation,
            _output: &mut ToolOutput,
            _conversation: &mut Conversation,
        ) {
            self.order.lock().unwrap().push(self.index + 10);
        }
    }

    let mut registry = HookRegistry::new();
    registry.register(Arc::new(OrderHook {
        index: 0,
        order: order.clone(),
    }));
    registry.register(Arc::new(OrderHook {
        index: 1,
        order: order.clone(),
    }));

    let invocation = make_invocation("test");
    let mut output = ToolOutput::new(json!("v"));
    let mut conversation = Conversation::new();

    registry
        .before_tool_call(&invocation, &conversation)
        .unwrap();
    registry.after_tool_call(&invocation, &mut output, &mut conversation);

    let recorded = order.lock().unwrap().clone();
    assert_eq!(recorded, vec![0, 1, 10, 11]);
}
