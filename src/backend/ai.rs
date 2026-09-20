//! AI 翻译后端（OpenAI 兼容协议）。
//!
//! 为什么只写一个实现就能支持一堆服务？因为国内的 DeepSeek、Kimi、
//! 智谱、通义，以及本地的 Ollama，绝大多数都提供 **OpenAI 兼容**的
//! `/v1/chat/completions` 接口。所以只要按 OpenAI 的格式说话，
//! 换个 `base_url` 和 `model` 就能切换服务商。
//!
//! 对比机器翻译，AI 后端的优势在于「懂上下文」：
//! - 能保留代码块、Markdown 结构
//! - 能按语境选词（技术文档 vs 小说 vs 口语）
//! - 能通过 prompt 定制风格（这些我们后面可以做 `--style` 参数）
//!
//! 代价：要 API Key、要花钱、比机翻慢。所以定位是「高质量首选」，
//! 而 MyMemory 是「永远能用的保底」。

use crate::backend::{http_client, net_err, Backend, Request};
use crate::error::{PoryError, Result};
use serde::{Deserialize, Serialize};

/// AI 后端配置
pub struct Ai {
    /// 接口根地址，比如 https://api.deepseek.com/v1
    base_url: String,
    /// API Key
    api_key: String,
    /// 模型名，比如 deepseek-chat、gpt-4o-mini
    model: String,
}

impl Ai {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            // 去掉末尾斜杠，避免拼出 //v1/chat...
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
        }
    }

    /// 构造给模型的指令。
    ///
    /// 这段 prompt 是 AI 翻译质量的关键。设计要点：
    /// - 明确「只输出译文」，否则模型爱加「以下是翻译：」这类废话
    /// - 明确「保持格式」，否则 Markdown 会被破坏
    /// - 用自然语言描述语种，而不是扔 "zh-CN" 这种代码
    fn build_prompt(req: &Request) -> String {
        let from = req.from.to_name();
        let to = req.to.to_name();

        if req.from.is_auto() {
            format!(
                "你是一个翻译引擎。请把用户给出的文本翻译成{to}。\n\
                 要求：\n\
                 1. 只输出译文本身，不要任何解释、不要加引号、不要写「翻译如下」之类的话\n\
                 2. 严格保留原文的换行、缩进、Markdown 标记、代码块和占位符\n\
                 3. 代码、变量名、专有名词保持原样不翻译\n\
                 4. 保持原文的语气和风格"
            )
        } else {
            format!(
                "你是一个翻译引擎。请把用户给出的{from}文本翻译成{to}。\n\
                 要求：\n\
                 1. 只输出译文本身，不要任何解释、不要加引号、不要写「翻译如下」之类的话\n\
                 2. 严格保留原文的换行、缩进、Markdown 标记、代码块和占位符\n\
                 3. 代码、变量名、专有名词保持原样不翻译\n\
                 4. 保持原文的语气和风格"
            )
        }
    }
}

/// OpenAI 兼容协议的请求体（只声明我们用得到的字段）
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    /// 温度给低一点，翻译要稳定不要发散
    temperature: f32,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

/// 响应体（同样只取需要的字段）
#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

impl Backend for Ai {
    fn name(&self) -> &str {
        "ai"
    }

    /// 把模型名纳入缓存键 —— 换模型后必须重新翻译，
    /// 否则会命中旧模型的译文（风格差异会很明显）。
    fn cache_detail(&self) -> String {
        self.model.clone()
    }

    fn max_chars(&self) -> usize {
        // 大模型的上下文窗口很大，但一次塞太多会慢且贵。
        // 8000 字符是个舒服的平衡点。
        8000
    }

    // trait 方法：装箱转发，见 mymemory.rs 里的说明
    fn translate<'a>(
        &'a self,
        req: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(self.do_translate(req))
    }
}

impl Ai {
    async fn do_translate(&self, req: Request) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let prompt = Self::build_prompt(&req);

        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                Message {
                    role: "system",
                    content: &prompt,
                },
                Message {
                    role: "user",
                    content: &req.text,
                },
            ],
            temperature: 0.3,
        };

        // 经统一的构造函数建客户端，才有超时兜底（见 backend/mod.rs 的说明）。
        // 大模型比机翻慢，30 秒是给它留的余量。
        let client = http_client()?;
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(net_err)?;

        let status = resp.status();
        if !status.is_success() {
            // 把服务端返回的错误正文读出来，方便定位（比如 key 无效、余额不足）
            let detail = resp.text().await.unwrap_or_default();
            return Err(PoryError::Backend(format!(
                "AI 后端返回 HTTP {status}：{}",
                detail.chars().take(300).collect::<String>()
            )));
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| PoryError::Parse(format!("AI 响应结构异常：{e}")))?;

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| PoryError::Backend("AI 后端返回了空回复".into()))
    }
}
