use crate::{ToolDefinition, ToolInvocation, error::BoxFutureResult};

pub type PolicyFuture<'a> = BoxFutureResult<'a, ToolDecision, std::convert::Infallible>;

pub trait ToolPolicy: Send + Sync {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a>;
}

#[derive(Clone, Debug)]
pub struct ToolPolicyRequest<'a> {
    pub invocation: &'a ToolInvocation,
    pub definition: Option<&'a ToolDefinition>,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolDecision {
    Allow,
    Deny { reason: String },
}

#[derive(Default)]
pub struct AllowAllPolicy;

impl ToolPolicy for AllowAllPolicy {
    fn evaluate<'a>(&'a self, _request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async { Ok(ToolDecision::Allow) })
    }
}

#[derive(Default)]
pub struct DenyAllMutatingPolicy;

impl ToolPolicy for DenyAllMutatingPolicy {
    fn evaluate<'a>(&'a self, request: ToolPolicyRequest<'a>) -> PolicyFuture<'a> {
        Box::pin(async move {
            match request.definition {
                Some(definition) if definition.annotations.mutating => Ok(ToolDecision::Deny {
                    reason: "mutating tools are denied".into(),
                }),
                _ => Ok(ToolDecision::Allow),
            }
        })
    }
}
