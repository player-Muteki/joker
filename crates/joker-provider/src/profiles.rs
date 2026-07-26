use crate::protocol::{Auth, Framing, Protocol, Route};

/// A known provider profile for quick configuration.
#[derive(Clone, Debug)]
pub struct ProviderProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api_key_env: &'static str,
    pub protocol: Protocol,
    pub framing: Framing,
}

impl ProviderProfile {
    pub fn default_auth(&self) -> Auth {
        match self.protocol {
            Protocol::ChatCompletions => Auth::bearer_from_env(self.api_key_env),
            Protocol::AnthropicMessages => Auth::api_key_from_env("x-api-key", self.api_key_env),
            Protocol::GoogleGemini => Auth::api_key_from_env("x-goog-api-key", self.api_key_env),
        }
    }

    pub fn into_route(&self, model: Option<&str>) -> Route {
        Route {
            id: self.id.into(),
            protocol: self.protocol.clone(),
            base_url: self.base_url.into(),
            auth: self.default_auth(),
            framing: self.framing.clone(),
            default_model: model.unwrap_or("").into(),
        }
    }
}

pub const ANTHROPIC: ProviderProfile = ProviderProfile {
    id: "anthropic",
    name: "Anthropic",
    base_url: "https://api.anthropic.com",
    api_key_env: "ANTHROPIC_API_KEY",
    protocol: Protocol::AnthropicMessages,
    framing: Framing::Sse,
};

pub const GOOGLE: ProviderProfile = ProviderProfile {
    id: "google",
    name: "Google",
    base_url: "https://generativelanguage.googleapis.com",
    api_key_env: "GOOGLE_GENERATIVE_AI_API_KEY",
    protocol: Protocol::GoogleGemini,
    framing: Framing::StreamableHttp,
};

pub const DEEPSEEK: ProviderProfile = ProviderProfile {
    id: "deepseek",
    name: "DeepSeek",
    base_url: "https://api.deepseek.com",
    api_key_env: "DEEPSEEK_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const ALIBABA: ProviderProfile = ProviderProfile {
    id: "alibaba",
    name: "Alibaba",
    base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    api_key_env: "ALIBABA_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const ZHIPUAI: ProviderProfile = ProviderProfile {
    id: "zhipuai",
    name: "ZhipuAI",
    base_url: "https://open.bigmodel.cn/api/paas/v4",
    api_key_env: "ZHIPUAI_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const MOONSHOT: ProviderProfile = ProviderProfile {
    id: "moonshot",
    name: "Moonshot",
    base_url: "https://api.moonshot.cn/v1",
    api_key_env: "MOONSHOT_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const BAIDU: ProviderProfile = ProviderProfile {
    id: "baidu",
    name: "Baidu",
    base_url: "https://qianfan.baidubce.com/v2",
    api_key_env: "BAIDU_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};
