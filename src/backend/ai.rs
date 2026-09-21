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
//! 同一个类型会按配置**实例化多个提供商**（见 config.rs 的 `[ai]` 段），
//! 所以实例要持有自己的名字（如 `zhipu` / `siliconflow`）——
//! 名字进缓存键与脚注，是区分提供商的唯一标识。
//!
//! 代价：要 API Key、要花钱（或用各家免费档）、比机翻慢。
//! 定位是「高质量首选」，传统机翻是「永远能用的保底」。

use crate::backend::{http_client, net_err, Backend, Request};
use crate::error::{PoryError, Result};
use serde::{Deserialize, Serialize};

/// AI 后端实例：一个 OpenAI 兼容提供商（名字 + 地址 + Key + 模型）
pub struct Ai {
    /// 本实例的名字（配置里的提供商名，如 "zhipu"）。
    /// 进缓存键与脚注 —— 换提供商换模型都必须是不同的缓存。
    name: String,
    /// 接口根地址，比如 https://api.deepseek.com/v1
    base_url: String,
    /// API Key
    api_key: String,
    /// 模型名，比如 deepseek-chat、gpt-4o-mini
    model: String,
    /// 思考模式开关。None = 不传参数（服务商默认）；
    /// Some(false/true) = 请求体带 enable_thinking。
    /// 影响输出 → 已通过 cache_detail 进缓存键。
    thinking: Option<bool>,
}

impl Ai {
    pub fn new(
        name: String,
        base_url: String,
        api_key: String,
        model: String,
        thinking: Option<bool>,
    ) -> Self {
        Self {
            name,
            // 去掉末尾斜杠，避免拼出 //v1/chat...
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
            thinking,
        }
    }

    /// 构造给模型的指令。
    ///
    /// 这段 prompt 是 AI 翻译质量的关键。设计要点：
    /// - 明确「只输出译文」，否则模型爱加「以下是翻译：」这类废话
    /// - 明确「保持格式」，否则 Markdown 会被破坏
    /// - 用自然语言描述语种，而不是扔 "zh-CN" 这种代码
    ///
    /// **互翻场景**（from=auto 且 `to_if_same` 存在）：AI 没有语言检测能力，
    /// 「请把中文翻译成中文」会让模型把原句换个说法当译文返回（实测
    /// Hunyuan-MT-7B 就这么干 —— 这是 2026-09-21 修掉的互翻失效 bug）。
    /// 解法是把判定交给模型：**「翻成 X；若原文已经是 X，则改译成 Y」**，
    /// 一次请求完成检测 + 翻译，不多花一次往返（轻量铁律）。
    /// 注意 `to_if_same` 本身就在缓存键里，所以这种 prompt 的产出不会
    /// 和普通请求互相污染。
    fn build_prompt(req: &Request) -> String {
        const REQUIREMENTS: &str = "\n要求：\n\
                 1. 只输出译文本身，不要任何解释、不要加引号、不要写「翻译如下」之类的话\n\
                 2. 严格保留原文的换行、缩进、Markdown 标记、代码块和占位符\n\
                 3. 代码、变量名、专有名词保持原样不翻译\n\
                 4. 保持原文的语气和风格";

        let to = req.to.to_name();

        if req.from.is_auto() {
            // 互翻开启时把「原文已是目标语」的处理写进指令（见上，修复中翻中）
            if let Some(alt) = &req.to_if_same {
                let alt = alt.to_name();
                return format!(
                    "你是一个翻译引擎。请把用户给出的文本翻译成{to}；\
                     若原文已经就是{to}，则改译成{alt}。{REQUIREMENTS}"
                );
            }
            format!(
                "你是一个翻译引擎。请把用户给出的文本翻译成{to}。{REQUIREMENTS}"
            )
        } else {
            let from = req.from.to_name();
            format!(
                "你是一个翻译引擎。请把用户给出的{from}文本翻译成{to}。{REQUIREMENTS}"
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
    /// 思考模式开关（混合推理模型如 Qwen3 系认这个字段；不支持的服务商
    /// 实测会安全忽略）。None 时整个字段省略，保持请求体与从前一致。
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_thinking: Option<bool>,
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
        // 动态返回实例名（配置里的提供商名，如 "zhipu"）。
        // 缓存键、回退脚注、警告文案里出现的都是它。
        &self.name
    }

    /// 把模型名纳入缓存键 —— 换模型后必须重新翻译，
    /// 否则会命中旧模型的译文（风格差异会很明显）。
    ///
    /// 思考模式同样影响输出，必须一起进键：
    /// `model`（未配置）/ `model/think` / `model/no-think`。
    /// 不进键的后果：关思考后同一段文字会命中开思考时的旧缓存 ——
    /// 慢的旧译文挡住新的快路径，与 v2→v3 修的「中翻中脏缓存」同型。
    fn cache_detail(&self) -> String {
        match self.thinking {
            None => self.model.clone(),
            Some(true) => format!("{}/think", self.model),
            Some(false) => format!("{}/no-think", self.model),
        }
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
            enable_thinking: self.thinking,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Lang;

    fn req(from: &str, to: &str, alt: Option<&str>) -> Request {
        Request {
            text: "hello".to_string(),
            from: Lang::parse(from).unwrap(),
            to: Lang::parse(to).unwrap(),
            to_if_same: alt.map(|l| Lang::parse(l).unwrap()),
        }
    }

    /// 互翻场景：备用语言必须写进指令 —— 这是「中翻中」bug 的守门测试。
    /// AI 没有检测能力，不写明「若原文已是 X 则改译 Y」就会照原样翻。
    #[test]
    fn 互翻时_prompt_写明备用语言() {
        let p = Ai::build_prompt(&req("auto", "zh-CN", Some("en")));
        assert!(p.contains("翻译成简体中文"), "{p}");
        assert!(p.contains("若原文已经就是简体中文"), "{p}");
        assert!(p.contains("改译成英语"), "{p}");
    }

    /// 未开启互翻（to_if_same 为空）时不提备用语言 —— prompt 不变多东西
    #[test]
    fn 非互翻的_auto_不提备用语言() {
        let p = Ai::build_prompt(&req("auto", "en", None));
        assert!(!p.contains("若原文已经就是"), "{p}");
        assert!(p.contains("翻译成英语"), "{p}");
    }

    /// 显式源语言：prompt 里带上源语种描述
    #[test]
    fn 显式源语言的_prompt_带源语种() {
        let p = Ai::build_prompt(&req("zh-CN", "en", None));
        assert!(p.contains("把用户给出的简体中文文本翻译成英语"), "{p}");
    }

    /// 思考开关进缓存键：三态必须产出三个不同的键后缀，
    /// 否则关思考会命中开思考时的慢缓存（与中翻中脏缓存同型的事故）
    #[test]
    fn 思考开关进缓存键() {
        let mk = |th: Option<bool>| {
            Ai::new(
                "sf:Qwen3-8B".into(),
                "https://x/v4".into(),
                "k".into(),
                "Qwen3-8B".into(),
                th,
            )
        };
        assert_eq!(mk(None).cache_detail(), "Qwen3-8B");
        assert_eq!(mk(Some(false)).cache_detail(), "Qwen3-8B/no-think");
        assert_eq!(mk(Some(true)).cache_detail(), "Qwen3-8B/think");
        assert_ne!(mk(None).cache_detail(), mk(Some(false)).cache_detail());
    }
}
