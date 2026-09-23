//! 微软翻译后端（Edge 免鉴权端点）。
//!
//! 用的是 Edge 浏览器翻译功能背后的接口：不需要 API Key、不需要注册，
//! 只要网络能访问 `edge.microsoft.com`（国内直连可达）。
//!
//! ⚠️ 两点必须知道：
//! 1. **非官方接口，无稳定性承诺。** 微软 2026-08 刚下线过配套的令牌接口
//!    `edge.microsoft.com/translate/auth`（永久 404），导致全网旧教程失效。
//!    本文件按 2026-09-21 实测可用的形态实现；哪天它也变了，pory 的回退链
//!    会自动落到下一个传统后端 —— 这正是双模式路由设计的意义。
//! 2. 请求形态有三处反直觉（都踩过 400）：
//!    - body 是**纯字符串数组** `["原文"]`，不是 Azure 文档那种 `[{"Text":"..."}]`
//!    - 查询参数必须带 `isEnterpriseClient=false`
//!    - 必须带浏览器 User-Agent
//!
//! 接口形态（实测）：
//! POST https://edge.microsoft.com/translate/translatetext
//!          ?to=zh-Hans&isEnterpriseClient=false[&from=en]
//! body: ["Hello, world!"]
//!
//! 返回：[{"detectedLanguage":{"language":"en","score":1.0},
//!        "translations":[{"text":"你好，世界！","to":"zh-Hans",...}]}]

use crate::backend::{
    http_client, looks_untranslated, net_err, same_primary_language, Backend, Request,
};
use crate::error::{PoryError, Result};
use serde_json::Value;

const ENDPOINT: &str = "https://edge.microsoft.com/translate/translatetext";

/// 浏览器 UA。端点要求请求头里有 User-Agent（浏览器环境天然满足），
/// 值本身没有强校验，取一个常见 Edge 指纹即可。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";

/// 微软翻译后端。无附加配置（免 Key），构造函数收 cfg 是为了未来留扩展位。
pub struct MsEdge;

impl MsEdge {
    pub fn new() -> Self {
        Self
    }

    /// 拼请求 URL。
    ///
    /// auto 模式**干脆不带 from 参数**（而不是传 `from=auto`）——
    /// 实测不带时微软自己检测并返回 `detectedLanguage`；
    /// 拆成纯函数是为了单测不发网络也能钉住 URL 形态。
    fn build_url(from: Option<&str>, to: &str) -> String {
        let mut url = format!("{ENDPOINT}?to={to}&isEnterpriseClient=false");
        if let Some(f) = from {
            url.push_str(&format!("&from={f}"));
        }
        url
    }
}

impl Default for MsEdge {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for MsEdge {
    fn name(&self) -> &str {
        "msedge"
    }

    fn max_chars(&self) -> usize {
        // 端点单次实测上限约 5 万字符，但对 CLI 翻译场景 5000 已经非常宽裕；
        // 取保守值还能让超长文本切分后逐块回显更整齐。
        5000
    }

    // trait 方法：装箱转发，见 mymemory.rs 里的说明
    fn translate<'a>(
        &'a self,
        req: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(self.do_translate(req))
    }
}

impl MsEdge {
    async fn do_translate(&self, req: Request) -> Result<String> {
        // to 必须转换（zh-CN → zh-Hans）；from 仅在非 auto 时带上
        let to = req.to.to_microsoft();
        let from = (!req.from.is_auto()).then(|| req.from.to_microsoft());

        // 源语言明确且与目标同语种 → 本地就能判掉，不必打扰服务器
        // （与 mymemory 同一条纪律：能本地判断的别问服务器）
        if let Some(f) = from.as_deref() {
            if same_primary_language(f, &to) {
                eprintln!("⚠ 源语言与目标语言相同（{}），原样返回", req.from.code());
                return Ok(req.text.clone());
            }
        }

        let (text, detected) = self.fetch(&req.text, from.as_deref(), &to).await?;

        // ── auto 模式下撞上「原文已是目标语言」→ 换向 ──
        // 实测（2026-09-22）：这种请求端点不会报错也不会换向，它老老实实
        // 「把中文翻成中文」—— 返回的译文就是原文。不处理的话，默认链上
        // 「中文进」会静默拿到中文，用户以为翻了。
        // 判据两条：端点自报的 detectedLanguage，或译文与原文完全相同。
        if req.from.is_auto() && same_language_hit(&detected, &to, &text, &req.text) {
            if let Some(alt) = &req.to_if_same {
                let alt_code = alt.to_microsoft();
                // 换向只花这一次请求；换向仍拿不到就走原文，不再纠缠
                let (swapped, _) = self.fetch(&req.text, None, &alt_code).await?;
                return Ok(swapped);
            }
            eprintln!(
                "⚠ 原文已经是目标语言（{}），原样返回",
                detected.unwrap_or(to)
            );
            return Ok(req.text.clone());
        }

        Ok(text)
    }

    /// 发一次请求，返回 `(译文, 检测到的源语言)`。
    ///
    /// 拆出来是因为「换向」要再发一次同样的请求、只换目标语言 ——
    /// 收发逻辑只该有一份。
    async fn fetch(
        &self,
        text: &str,
        from: Option<&str>,
        to: &str,
    ) -> Result<(String, Option<String>)> {
        let url = Self::build_url(from, to);

        // body 是纯字符串数组 —— 用 Azure 文档的 [{"Text":...}] 会吃 400
        let body = serde_json::json!([text]);

        // 经统一的构造函数发请求，才有超时兜底（见 backend/mod.rs 的说明）。
        let resp = http_client()?
            .post(&url)
            .header("User-Agent", USER_AGENT)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(net_err)?;

        let status = resp.status();
        if !status.is_success() {
            return Err(PoryError::Backend(format!(
                "微软翻译端点返回 HTTP {status}。非官方接口，若持续失败请换用其他传统后端"
            )));
        }

        let raw = resp
            .text()
            .await
            .map_err(|e| PoryError::Parse(format!("读取微软翻译响应失败：{e}")))?;

        // 防一手：风控/拦截时可能返回 HTML 页而不是 JSON，直接解析会报看不懂的错
        let trimmed = raw.trim_start();
        if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") {
            return Err(PoryError::Backend(
                "微软翻译端点返回了 HTML 页面（可能是风控拦截），请稍后重试".into(),
            ));
        }

        let json: Vec<Value> = serde_json::from_str(&raw)
            .map_err(|e| PoryError::Parse(format!("微软翻译响应不是合法 JSON：{e}")))?;

        let first = json
            .first()
            .ok_or_else(|| PoryError::Backend("微软翻译端点返回了空数组".into()))?;

        // 结构：数组第一项的 translations[0].text
        let text = first
            .get("translations")
            .and_then(|t| t.as_array())
            .and_then(|t| t.first())
            .and_then(|t| t.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        if text.is_empty() {
            return Err(PoryError::Backend("微软翻译端点返回了空译文".into()));
        }

        // 端点会回 detectedLanguage.language（如 "zh-Hans"）—— 同语种判断靠它
        let detected = first
            .get("detectedLanguage")
            .and_then(|d| d.get("language"))
            .and_then(|l| l.as_str())
            .map(|s| s.to_string());

        Ok((text.to_string(), detected))
    }
}

/// 这次响应是不是「原文已经是目标语言」。
///
/// 两条判据取或：端点自报的检测语种与目标同主语言，或译文与原文完全一致
/// （后者兜住「端点在检测上不吭声」的情况）。判据的来龙去脉见 mod.rs。
fn same_language_hit(detected: &Option<String>, to: &str, translated: &str, source: &str) -> bool {
    detected
        .as_deref()
        .is_some_and(|d| same_primary_language(d, to))
        || looks_untranslated(translated, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_形态含企业参数() {
        let url = MsEdge::build_url(Some("en"), "zh-Hans");
        assert_eq!(
            url,
            "https://edge.microsoft.com/translate/translatetext?to=zh-Hans&isEnterpriseClient=false&from=en"
        );
    }

    /// auto 时不带 from 参数 —— 传 from=auto 实测不行，检测要靠省略参数
    #[test]
    fn auto_时省略_from() {
        let url = MsEdge::build_url(None, "zh-Hans");
        assert_eq!(
            url,
            "https://edge.microsoft.com/translate/translatetext?to=zh-Hans&isEnterpriseClient=false"
        );
        assert!(!url.contains("from="));
    }

    /// 语言映射：微软要 zh-Hans / zh-Hant
    #[test]
    fn 响应解析取_translations_首项() {
        let raw = r#"[{"detectedLanguage":{"language":"en","score":1.0},"translations":[{"text":"你好，世界！这是个测试。","to":"zh-Hans","sentLen":{"srcSentLen":[14,15],"transSentLen":[6,6]}}]}]"#;
        let json: Vec<Value> = serde_json::from_str(raw).unwrap();
        let text = json
            .first()
            .and_then(|item| item.get("translations"))
            .and_then(|t| t.as_array())
            .and_then(|t| t.first())
            .and_then(|t| t.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        assert_eq!(text, "你好，世界！这是个测试。");
    }

    /// 后端名就是缓存键与脚注里出现的名字，钉死它
    #[test]
    fn 后端名是_msedge() {
        assert_eq!(MsEdge::new().name(), "msedge");
    }
}
