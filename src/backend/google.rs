//! Google 翻译后端（非官方接口）。
//!
//! 用的是 Google 翻译网页版背后的 `translate_a/single` 接口：
//! 不需要 API Key、不需要登录，只要网络能访问 `translate.googleapis.com`。
//!
//! ⚠️ 三点必须知道：
//! 1. **国内直连不通**，需要代理。这是它没法做默认后端的原因。
//! 2. 它是非官方接口，没有稳定性承诺，可能改参数或加风控。
//! 3. 高频调用可能触发验证码，返回 HTML 而不是 JSON —— 代码里要防这一手。
//!
//! 接口形态：
//! GET https://translate.googleapis.com/translate_a/single
//!     ?client=gtx&dt=t&sl=auto&tl=en&q=文本
//!
//! 返回：[[["译文","原文",null,null,10]],null,"zh-CN",...]
//!      译文是 res[0] 里所有片段的 [0] 拼接。

use crate::backend::{
    http_client, looks_untranslated, net_err, same_primary_language, Backend, Request,
};
use crate::error::{PoryError, Result};

const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";

/// Google 后端。可选一个自定义代理地址（比如自己搭的 Cloudflare Worker）。
pub struct Google {
    /// 自定义接口地址。不填就用官方域名。
    /// 国内用户可以在配置里填自己反代的地址。
    endpoint: String,
}

impl Google {
    pub fn new(endpoint: Option<String>) -> Self {
        Self {
            endpoint: endpoint.unwrap_or_else(|| ENDPOINT.to_string()),
        }
    }
}

impl Backend for Google {
    fn name(&self) -> &str {
        "google"
    }

    fn max_chars(&self) -> usize {
        // Google 单次大约能吃 5000 字符，保守取 4000
        4000
    }

    // trait 方法：装箱转发，见 mymemory.rs 里的说明
    fn translate<'a>(
        &'a self,
        req: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(self.do_translate(req))
    }
}

impl Google {
    async fn do_translate(&self, req: Request) -> Result<String> {
        // sl=auto 时 Google 自己会检测，直接用 "auto" 即可
        let sl = if req.from.is_auto() {
            "auto".to_string()
        } else {
            req.from.code().to_string()
        };

        let tl = req.to.code().to_string();

        // 源语言明确且与目标同语种 → 本地判掉，不必打扰服务器
        // （与 mymemory 同一条纪律：能本地判断的别问服务器）
        if !req.from.is_auto() && same_primary_language(req.from.code(), &tl) {
            eprintln!("⚠ 源语言与目标语言相同（{}），原样返回", req.from.code());
            return Ok(req.text.clone());
        }

        let (text, detected) = self.fetch(&req.text, &sl, &tl).await?;

        // ── auto 模式下撞上「原文已是目标语言」→ 换向 ──
        // 未在本机实测（国内 429），但同族的免 Key 端点（msedge / transmart）
        // 都实测「不换向、原样返回」，所以按同样口径处理：
        // 检测码与目标同主语言，或译文与原文完全一致时就换向。
        let same = detected
            .as_deref()
            .is_some_and(|d| same_primary_language(d, &tl))
            || looks_untranslated(&text, &req.text);
        if req.from.is_auto() && same {
            if let Some(alt) = &req.to_if_same {
                let (swapped, _) = self.fetch(&req.text, "auto", alt.code()).await?;
                return Ok(swapped);
            }
            eprintln!("⚠ 原文已经是目标语言，原样返回");
            return Ok(req.text.clone());
        }

        Ok(text)
    }

    /// 发一次请求，返回 `(译文, 检测到的源语言)`。
    ///
    /// 抽出来是因为「换向」要再发一次、只换目标语言；收发逻辑只该有一份。
    async fn fetch(
        &self,
        text: &str,
        sl: &str,
        tl: &str,
    ) -> Result<(String, Option<String>)> {
        let url = format!(
            "{}?client=gtx&dt=t&sl={}&tl={}&q={}",
            self.endpoint,
            urlencoding::encode(sl),
            urlencoding::encode(tl),
            urlencoding::encode(text),
        );

        // 经统一的构造函数发请求，才有超时兜底（见 backend/mod.rs 的说明）。
        // 直接用 reqwest::get() 会无限等待挂起的连接。
        let resp = http_client()?.get(&url).send().await.map_err(net_err)?;

        if !resp.status().is_success() {
            return Err(PoryError::Backend(format!(
                "Google 返回 HTTP {}。若为 403，多半是被风控或需要代理。",
                resp.status()
            )));
        }

        let raw = resp
            .text()
            .await
            .map_err(|e| PoryError::Parse(format!("读取 Google 响应失败：{e}")))?;

        // 防一手：风控时会返回 HTML 验证码页，直接 JSON 解析会报一堆看不懂的错
        let trimmed = raw.trim_start();
        if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") {
            return Err(PoryError::Backend(
                "Google 返回了 HTML 页面（多半是验证码或风控拦截），请稍后重试或改用其他后端".into(),
            ));
        }

        let json: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| PoryError::Parse(format!("Google 响应不是合法 JSON：{e}")))?;

        // 结构：res[0] 是一个数组，每项是 [译文片段, 原文片段, ...]
        // 长文本会被切成多段，必须全部拼起来。
        let segments = json[0]
            .as_array()
            .ok_or_else(|| PoryError::Parse("Google 响应结构异常：第一项不是数组".into()))?;

        let mut out = String::new();
        for seg in segments {
            // 每段本身也是数组，第 0 位才是译文。有些段是 null，跳过。
            if let Some(piece) = seg.as_array().and_then(|a| a.first()).and_then(|v| v.as_str()) {
                out.push_str(piece);
            }
        }

        if out.is_empty() {
            return Err(PoryError::Backend("Google 返回了空译文".into()));
        }

        // res[2] 是检测到的源语言码（如 "zh-CN"）：结构注释里那个 `null,"zh-CN"`
        let detected = json
            .get(2)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok((out, detected))
    }
}
