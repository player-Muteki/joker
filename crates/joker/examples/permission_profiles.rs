use joker::{PermissionEngine, ToolName};

fn print_profile(engine: &PermissionEngine, name: &str) {
    println!("agent: {name}");
    for tool in [
        "read_file",
        "write_file",
        "shell",
        "web_search",
        "memory_read",
    ] {
        let tool_name = ToolName::new(tool);
        let decision = engine.evaluate(name, &tool_name, true, None);
        println!("  {tool}: {decision:?}");
    }
    println!();
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut engine = PermissionEngine::new();

    for profile in joker::builtin_agent_profiles(std::path::Path::new("")) {
        engine.register(profile);
    }

    print_profile(&engine, "plan");
    print_profile(&engine, "build");
    print_profile(&engine, "yolo");

    println!("--- session grant demonstration ---");
    let tool = ToolName::new("web_search");
    let before = engine.evaluate("yolo", &tool, true, None);
    println!("before grant: {before:?}");
    engine.grant_session("yolo", tool.clone());
    let after = engine.evaluate("yolo", &tool, true, None);
    println!("after grant:  {after:?}");
}
