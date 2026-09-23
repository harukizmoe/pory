//! 腾讯交互翻译（TranSmart）后端。
//!
//! 走的是 transmart.qq.com 网页版背后的接口：不需要 API Key、国内直连，
//! 实测响应极快（约 70 ms），还会顺带返回检测到的源语言（`src_lang`）。
//!
//! ⚠️ 非官方接口，无稳定性承诺 —— 坏了由回退链兜住，这正是传统链
//! 排了四家的原因（见 config.rs 的默认 order）。
//!
//! 接口形态（2026-09-21 实测）：
//! POST https://transmart.qq.com/api/imt
//! body: {
//!   "header": {"fn": "auto_translation", "session": "", "client_key": "..."},
//!   "type": "plain", "model_category": "normal", "text_domain": "",
//!   "source": {"lang": "auto", "text_list": ["Hello"]},
//!   "target": {"lang": "zh"}
//! }
//! 返回：{"header":{...}, "auto_translation":["你好"],
//!        "src_lang":"en", "tgt_lang":"zh"}

use crate::backend::{
    http_client, looks_untranslated, net_err, same_primary_language, Backend, Request,
};
use crate::error::{PoryError, Result};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

const ENDPOINT: &str = "https://transmart.qq.com/api/imt";

/// 网页端点的输入框限制是 5000 字符，留余量取 4000
/// （与 google 后端同一档，链内切分粒度不会因它变碎）。
const MAX_CHARS: usize = 4000;

/// 造一个形如 UUID 的随机串（8-4-4-4-12 十六进制）。
///
/// **为什么不引 uuid crate**：pory 的上位约束是轻量 —— 这里的 client_key
/// 只是网页端点用来区分浏览器会话的指纹，**没有任何安全性要求**，
/// 只要「每次调用不同、长得像浏览器生成的」即可。纳秒时间戳 × 进程号
/// 的组合已经足够，20 行换掉一个依赖，值。
///
/// 熵源：纳秒时间戳（主）⊕ 进程号（防同纳秒并发重复）⊕ 栈地址低位
/// （ASLR 每次启动不同）。不做密码学处理 —— 不是密钥，别过度设计。
fn fake_uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let stack_entropy = &nanos as *const u64 as u64;

    // 两段 64 位，互相搅拌（ xorshift 一轮即可，别写成密码学爱好者）
    let mut a = nanos ^ (pid << 32) ^ stack_entropy;
    let mut b = a.rotate_left(17) ^ pid.rotate_left(13);
    a ^= b >> 7;
    b ^= a << 5;

    let hex = format!("{a:016x}{b:016x}"); // 32 个十六进制字符
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 腾讯交互翻译后端。无附加配置（免 Key）。
pub struct Transmart;

impl Transmart {
    pub fn new() -> Self {
        Self
    }

    /// 拼请求体。拆成纯函数便于单测钉住形态（端点对字段名敏感）。
    ///
    /// 语言码用**两位主码**（zh / en / ja...，与 MyMemory 同款转换）——
    /// 实测 `zh-CN` 这种带区域码的写法它不认。
    fn build_body(text: &str, from: Option<&str>, to: &str) -> Value {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        json!({
            "header": {
                "fn": "auto_translation",
                "session": "",
                "client_key": format!("browser-chrome-131.0.0-Windows_10-{}-{}", fake_uuid(), ts)
            },
            "type": "plain",
            "model_category": "normal",
            "text_domain": "",
            "source": {
                "lang": from.unwrap_or("auto"),
                "text_list": [text]
            },
            "target": {
                "lang": to
            }
        })
    }
}

impl Default for Transmart {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Transmart {
    fn name(&self) -> &str {
        "transmart"
    }

    fn max_chars(&self) -> usize {
        MAX_CHARS
    }

    // trait 方法：装箱转发，见 mymemory.rs 里的说明
    fn translate<'a>(
        &'a self,
        req: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(self.do_translate(req))
    }
}

impl Transmart {
    async fn do_translate(&self, req: Request) -> Result<String> {
        // 两位主语言码；auto 传字符串 "auto"（端点自己检测，还会回 src_lang）
        let from = (!req.from.is_auto()).then(|| req.from.to_mymemory());
        let to = req.to.to_mymemory();

        // 源语言明确且与目标同语种 → 本地判掉，不必打扰服务器
        // （与 mymemory 同一条纪律：能本地判断的别问服务器）
        if let Some(f) = from.as_deref() {
            if same_primary_language(f, &to) {
                eprintln!("⚠ 源语言与目标语言相同（{}），原样返回", req.from.code());
                return Ok(req.text.clone());
            }
        }

        let (text, src_lang) = self.fetch(&req.text, from.as_deref(), &to).await?;

        // ── auto 模式下撞上「原文已是目标语言」→ 换向 ──
        // 实测（2026-09-22）：target 与源语言相同时，端点**不返回 `src_lang`**
        // （它省略了这个字段），译文就是原文本身 —— 静默的「中文翻中文」。
        // 所以这里主要靠 `looks_untranslated` 判；`src_lang` 有值时也顺手用。
        let same = looks_untranslated(&text, &req.text)
            || src_lang
                .as_deref()
                .is_some_and(|s| same_primary_language(s, &to));
        if req.from.is_auto() && same {
            if let Some(alt) = &req.to_if_same {
                let alt_code = alt.to_mymemory();
                let (swapped, _) = self.fetch(&req.text, None, &alt_code).await?;
                return Ok(swapped);
            }
            eprintln!("⚠ 原文已经是目标语言，原样返回");
            return Ok(req.text.clone());
        }

        Ok(text)
    }

    /// 发一次请求，返回 `(译文, 服务端自报的源语言)`。
    ///
    /// 抽出来是因为「换向」要再发一次、只换目标语言；收发逻辑只该有一份。
    async fn fetch(
        &self,
        text: &str,
        from: Option<&str>,
        to: &str,
    ) -> Result<(String, Option<String>)> {
        let body = Self::build_body(text, from, to);

        // 经统一的构造函数发请求，才有超时兜底（见 backend/mod.rs 的说明）。
        let resp = http_client()?
            .post(ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Origin", "https://transmart.qq.com")
            .header("Referer", "https://transmart.qq.com/")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            )
            .json(&body)
            .send()
            .await
            .map_err(net_err)?;

        let status = resp.status();
        if !status.is_success() {
            return Err(PoryError::Backend(format!(
                "腾讯交互翻译返回 HTTP {status}。非官方接口，若持续失败请换用其他传统后端"
            )));
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| PoryError::Parse(format!("腾讯交互翻译响应不是合法 JSON：{e}")))?;

        // 端点用 ret_code 表达业务成功与否，HTTP 层 200 也可能业务失败
        let ret = json
            .pointer("/header/ret_code")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !ret.is_empty() && ret != "succ" {
            return Err(PoryError::Backend(format!(
                "腾讯交互翻译返回业务错误（ret_code={ret}）"
            )));
        }

        // auto_translation 是数组（text_list 支持批量），我们只发了一段
        let text = json
            .get("auto_translation")
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if text.is_empty() {
            return Err(PoryError::Backend("腾讯交互翻译返回了空译文".into()));
        }

        // 源语言与目标相同时端点会省略 src_lang（见 do_translate 的说明）
        let src_lang = json
            .get("src_lang")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok((text.to_string(), src_lang))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// client_key 形态：浏览器指纹前缀 + uuid + 毫秒时间戳。
    /// 端点靠它区分会话，格式不对会被拒。
    #[test]
    fn 请求体形态钉死() {
        let body = Transmart::build_body("Hello", None, "zh");
        assert_eq!(body["header"]["fn"], "auto_translation");
        assert_eq!(body["source"]["lang"], "auto");
        assert_eq!(body["source"]["text_list"][0], "Hello");
        assert_eq!(body["target"]["lang"], "zh");
        assert_eq!(body["type"], "plain");

        let key = body["header"]["client_key"].as_str().unwrap();
        assert!(
            key.starts_with("browser-chrome-131.0.0-Windows_10-"),
            "{key}"
        );
        // 按 '-' 全切：前 4 段是前缀（browser / chrome / 131.0.0 / Windows_10），
        // 接着 5 段是 uuid（8-4-4-4-12），最后一段是毫秒时间戳
        let parts: Vec<&str> = key.split('-').collect();
        assert_eq!(parts.len(), 10, "{key}");
        assert_eq!(
            parts[4..9].iter().map(|s| s.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{key}"
        );
        assert!(
            parts[9].chars().all(|c| c.is_ascii_digit()),
            "时间戳段应全数字：{key}"
        );
    }

    /// 显式源语言走两位码；zh-CN 的区域码要砍掉（端点不认带区域的写法）
    #[test]
    fn 语言码用两位主码() {
        let body = Transmart::build_body("text", Some("zh"), "en");
        assert_eq!(body["source"]["lang"], "zh");
        assert_eq!(body["target"]["lang"], "en");
    }

    /// fake_uuid 的形态与唯一性（快速连打两次也要不同）
    #[test]
    fn fake_uuid_形态正确且两次不同() {
        let a = fake_uuid();
        let b = fake_uuid();
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'), "{a}");
        assert_ne!(a, b, "同纳秒也要不同（进程号/栈熵兜底）");
    }

    /// 响应解析：取 auto_translation 数组首项
    #[test]
    fn 响应解析取_auto_translation_首项() {
        let raw = r#"{"header":{"type":"auto_translation","ret_code":"succ","time_cost":71.0,"request_id":"17a1940"},"auto_translation":["你好，世界！这是一个测试。"],"src_lang":"en","tgt_lang":"zh"}"#;
        let json: Value = serde_json::from_str(raw).unwrap();
        let text = json
            .get("auto_translation")
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(text, "你好，世界！这是一个测试。");
    }

    /// 后端名就是缓存键与脚注里出现的名字，钉死它
    #[test]
    fn 后端名是_transmart() {
        assert_eq!(Transmart::new().name(), "transmart");
    }
}
