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
use crate::config::{is_valid_provider_name, Config};
use crate::error::{PoryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
    /// 是否启用思考；默认 false，影响请求内容和缓存键。
    thinking: bool,
}

impl Ai {
    pub fn new(
        name: String,
        base_url: String,
        api_key: String,
        model: String,
        thinking: bool,
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
    /// - **用户正文一律包在 `<text>` 标签里**：2026-09-22 实测，不带标签时
    ///   以 `Translate ...` 开头的输入会让模型把系统指令和正文混起来，
    ///   直接把「要求 1-4」复述出来当译文（Hunyuan-MT-7B 稳定复现）
    ///
    /// **互翻场景**（from=auto 且 `to_if_same` 存在）：AI 没有语言检测能力，
    /// 「请把中文翻译成中文」会让模型把原句换个说法当译文返回（实测
    /// Hunyuan-MT-7B 就这么干 —— 这是 2026-09-21 修掉的互翻失效 bug）。
    /// 解法是把判定交给模型，但**写法必须是两条并列规则，不能用分号串成一句**：
    /// 2026-09-22 实测，「翻译成 A；若原文已是 A 则改译成 B」会让模型对
    /// **英文输入**只做润色（`The weather is nice...` → 加个逗号原样返回），
    /// 也就是「翻成主语言」这条路径整个失效 —— 而样本全是中文的测评
    /// 恰好只覆盖了另一条路径，所以一直没暴露。
    /// 拆成「先判断，再按两条规则走」之后，两个方向都稳定。
    ///
    /// 注意 `to_if_same` 本身就在缓存键里，所以这种 prompt 的产出不会
    /// 和普通请求互相污染。
    fn build_prompt(req: &Request) -> String {
        const REQUIREMENTS: &str = "\n要求：\n\
                 1. 只输出译文本身，不要任何解释、不要加引号、不要写「翻译如下」之类的话\n\
                 2. 严格保留原文的换行、缩进、Markdown 标记、代码块和占位符\n\
                 3. 代码、变量名、专有名词保持原样不翻译\n\
                 4. 保持原文的语气和风格";

        let to = req.to.to_name();

        let head = if req.from.is_auto() {
            // 互翻开启：把两种语言**抽象成 A / B 两个符号**再谈规则。
            //
            // 2026-09-22 实测（Hunyuan-MT-7B，每格 2 次采样）：
            //   旧写法「翻译成简体中文；若原文已经就是简体中文，则改译成英语」→ 英文输入 0/6
            //   本写法（A/B 命名）→ 英文输入 6/6、中文输入 2/2
            // 原因推测：旧写法里「简体中文」同时扮演「目标」和「条件」，模型容易
            // 把它读成「输出语言 = 简体中文」的对立面，于是对英文输入直接润色回英文；
            // 抽成符号后两个角色不再撞名。别改回带分号的直觉写法。
            if let Some(alt) = &req.to_if_same {
                let alt = alt.to_name();
                format!(
                    "你是一个翻译引擎。设 A = {to}，B = {alt}。\n\
                     把 <text> 标签里的内容翻译成 A；但如果内容已经是 A，则翻译成 B。\n\
                     译文本身不要带 <text> 标签。"
                )
            } else {
                format!(
                    "你是一个翻译引擎。请把 <text> 标签里的内容翻译成{to}。\
                     译文本身不要带 <text> 标签。"
                )
            }
        } else {
            let from = req.from.to_name();
            format!(
                "你是一个翻译引擎。请把 <text> 标签里的{from}内容翻译成{to}。\
                 译文本身不要带 <text> 标签。"
            )
        };

        format!("{head}{REQUIREMENTS}")
    }
}
/// 按配置顺序生成可用的 AI 模型，并保留跳过原因供调用方展示。
#[derive(Default)]
pub(crate) struct AiCandidates {
    pub(crate) models: Vec<Ai>,
    pub(crate) no_key: Vec<String>,
    pub(crate) skipped: Vec<(String, String)>,
}

/// 翻译和词典共用同一份提供商、模型排序与校验规则。
pub(crate) fn configured(cfg: &Config) -> AiCandidates {
    let mut candidates = AiCandidates::default();
    let mut seen = HashSet::new();

    for name in &cfg.ai.order {
        if !seen.insert(name) {
            continue;
        }
        if !is_valid_provider_name(name) {
            candidates.skipped.push((
                name.clone(),
                "名字不合法（只允许小写字母 / 数字 / 连字符）".into(),
            ));
            continue;
        }
        let Some(provider) = cfg.ai.providers.get(name) else {
            candidates.skipped.push((
                name.clone(),
                "order 里列了名字，但 [ai] 下没有对应的配置表".into(),
            ));
            continue;
        };
        if provider.api_key.trim().is_empty() {
            candidates.no_key.push(name.clone());
            continue;
        }
        if provider.models.is_empty() {
            candidates
                .skipped
                .push((name.clone(), "没有列出任何模型（models 为空）".into()));
            continue;
        }

        for model in &provider.models {
            candidates.models.push(Ai::new(
                format!("{name}:{model}"),
                provider.base_url.clone(),
                provider.api_key.clone(),
                model.clone(),
                provider.thinking,
            ));
        }
    }

    candidates
}

/// 判断译文是不是**在复述我们的指令**，而不是翻译。
///
/// 2026-09-22 加：模型（实测 Hunyuan-MT-7B）在指令与正文混淆时，会把
/// prompt 里的「要求 1-4」当成要处理的内容，逐条吐出来 —— 这是最恶劣的
/// 一种失败：**看着像正经输出，其实是彻底的垃圾**。
///
/// 判据刻意收窄：只认我们 prompt 里的独特措辞（中文原文 + 模型转写成英文
/// 后的常见说法），且**要命中两条以上**才算 —— 单条可能是用户真的在翻译
/// 一份提示词文档，那不该误伤。
///
/// 命中后由调用方返回错误（而不是把垃圾当译文交出去），于是回退链继续走、
/// 垃圾也不会进缓存。
fn looks_like_prompt_echo(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        // prompt 的中文原文措辞
        "只输出译文本身",
        "严格保留原文的换行",
        "专有名词保持原样",
        "保持原文的语气和风格",
        "不要写「翻译如下」",
        // 模型把指令转写成英文后的常见说法
        "only output the translated text",
        "strictly maintain the original text",
        "strictly preserve the original text",
        "proper nouns should remain",
        "preserve the original text",
    ];
    let lower = text.to_lowercase();
    let hits = MARKERS
        .iter()
        .filter(|m| lower.contains(&m.to_lowercase()))
        .count();
    hits >= 2
}

/// OpenAI 兼容协议的请求体（只声明我们用得到的字段）
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    /// 降低随机性，让翻译和结构化词条输出更稳定。
    temperature: f32,
    /// 显式发送开关，避免服务商的默认值意外开启思考模式。
    enable_thinking: bool,
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
    /// 思考模式影响输出，必须进入缓存键，避免开关状态互相命中。
    fn cache_detail(&self) -> String {
        if self.thinking {
            format!("{}/think", self.model)
        } else {
            format!("{}/no-think", self.model)
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
    /// 发送一组 OpenAI 兼容对话消息，返回模型原始文本。
    pub(crate) async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                Message {
                    role: "system",
                    content: system,
                },
                Message {
                    role: "user",
                    content: user,
                },
            ],
            temperature: 0.3,
            enable_thinking: self.thinking,
        };

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
            .map(|choice| choice.message.content.trim().to_string())
            .filter(|content| !content.is_empty())
            .ok_or_else(|| PoryError::Backend("AI 后端返回了空回复".into()))
    }

    async fn do_translate(&self, req: Request) -> Result<String> {
        let prompt = Self::build_prompt(&req);
        // 正文包进 <text> 标签，与 prompt 里的说法对应。
        let user_content = format!("<text>{}</text>", req.text);
        let content = self.complete(&prompt, &user_content).await?;

        // 复述指令不是译文，宁可回退也不把它当结果交出去。
        if looks_like_prompt_echo(&content) {
            return Err(PoryError::Backend(format!(
                "{} 复述了翻译指令而不是翻译正文（已判为失败）。该模型的指令跟随不稳定，\
                 可考虑换一个模型，或在配置里关掉互翻（secondary = \"\"）",
                self.name
            )));
        }

        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Lang;
    #[test]
    fn thinking_flag_is_explicit_in_request() {
        let provider = crate::config::ProviderConfig::default();
        let request = ChatRequest {
            model: "test",
            messages: Vec::new(),
            temperature: 0.3,
            enable_thinking: provider.thinking,
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["enable_thinking"], serde_json::Value::Bool(false));

        let enabled = ChatRequest {
            model: "test",
            messages: Vec::new(),
            temperature: 0.3,
            enable_thinking: true,
        };
        let value = serde_json::to_value(enabled).unwrap();
        assert_eq!(value["enable_thinking"], serde_json::Value::Bool(true));
    }

    fn req(from: &str, to: &str, alt: Option<&str>) -> Request {
        Request {
            text: "hello".to_string(),
            from: Lang::parse(from).unwrap(),
            to: Lang::parse(to).unwrap(),
            to_if_same: alt.map(|l| Lang::parse(l).unwrap()),
        }
    }

    /// 互翻场景：两个方向都必须写进指令 —— 这是「中翻中」bug 的守门测试。
    /// AI 没有检测能力，不写明「若原文已是 A 则改译 B」就会照原样翻。
    #[test]
    fn 互翻时_prompt_写明两个方向() {
        let p = Ai::build_prompt(&req("auto", "zh-CN", Some("en")));
        assert!(p.contains("设 A = 简体中文，B = 英语"), "{p}");
        assert!(p.contains("翻译成 A"), "{p}");
        assert!(p.contains("如果内容已经是 A，则翻译成 B"), "{p}");
    }

    /// **防回归**：两种语言必须先抽象成 A / B 再谈规则。
    /// 2026-09-22 实测：把语言名直接塞进条件句（「翻译成简体中文；若原文已经
    /// 就是简体中文，则改译成英语」）会让模型对**英文输入**只做润色，0/6 全败；
    /// 改成 A/B 命名后 6/6 通过。别退回旧写法。
    #[test]
    fn 互翻指令不把语言名塞进条件句() {
        let p = Ai::build_prompt(&req("auto", "zh-CN", Some("en")));
        assert!(
            !p.contains("翻译成简体中文；"),
            "退回旧写法会让英文输入失效：{p}"
        );
        // A/B 的定义必须出现在规则之前
        let def = p.find("设 A =").expect("缺 A/B 定义");
        let rule = p.find("翻译成 A").expect("缺规则");
        assert!(def < rule, "定义要在规则之前：{p}");
    }

    /// 正文必须包在 `<text>` 标签里 —— 不带标签时以 "Translate ..." 开头的
    /// 输入会被模型当成指令，直接把「要求」条目复述出来
    #[test]
    fn prompt_要求正文包在_text_标签里() {
        for (from, alt) in [("auto", Some("en")), ("auto", None), ("zh-CN", None)] {
            let p = Ai::build_prompt(&req(from, "en", alt));
            assert!(p.contains("<text>"), "{p}");
            assert!(p.contains("不要带 <text> 标签"), "{p}");
        }
    }

    /// 未开启互翻（to_if_same 为空）时不提备用语言 —— prompt 不变多东西
    #[test]
    fn 非互翻的_auto_不提备用语言() {
        let p = Ai::build_prompt(&req("auto", "en", None));
        assert!(!p.contains("设 A ="), "{p}");
        assert!(p.contains("翻译成英语"), "{p}");
    }

    /// 显式源语言：prompt 里带上源语种描述
    #[test]
    fn 显式源语言的_prompt_带源语种() {
        let p = Ai::build_prompt(&req("zh-CN", "en", None));
        assert!(
            p.contains("把 <text> 标签里的简体中文内容翻译成英语"),
            "{p}"
        );
    }

    /// 复述指令的判定：要命中两条以上才算，避免误伤「正在翻译一份提示词文档」
    #[test]
    fn 复述指令的译文会被判失败() {
        // 用户报的那种（模型把指令转写成英文后逐条吐出）
        let echo_en = "Requirement:\n\
            1. Only output the translated text itself; no explanations, no quotes, and no phrases like \"Translation as follows\".\n\
            2. Strictly maintain the original text's line breaks, indentation, Markdown formatting, code blocks, and placeholders.";
        assert!(looks_like_prompt_echo(echo_en));

        // 中文原文照搬
        assert!(looks_like_prompt_echo(
            "要求：1. 只输出译文本身，不要任何解释 2. 严格保留原文的换行、缩进"
        ));

        // 正常译文不误判
        assert!(!looks_like_prompt_echo(
            "一款终端翻译器：开箱即用，也可以接上你自己选的 AI。"
        ));
        assert!(!looks_like_prompt_echo("The terminal is quiet."));

        // 只命中一条 → 不算（可能是用户在翻译提示词文档）
        assert!(!looks_like_prompt_echo("这份提示词要求：只输出译文本身。"));
    }

    /// 缓存键区分默认关闭与显式开启的思考模式。
    #[test]
    fn 思考开关进缓存键() {
        let mk = |thinking: bool| {
            Ai::new(
                "sf:Qwen3-8B".into(),
                "https://x/v4".into(),
                "k".into(),
                "Qwen3-8B".into(),
                thinking,
            )
        };
        assert_eq!(mk(false).cache_detail(), "Qwen3-8B/no-think");
        assert_eq!(mk(true).cache_detail(), "Qwen3-8B/think");
        assert_ne!(mk(false).cache_detail(), mk(true).cache_detail());
    }
}
